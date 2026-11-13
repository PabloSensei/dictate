# Builds the release zips that people download from GitHub Releases. Run from PowerShell:
#   .\package.ps1              both variants
#   .\package.ps1 -Variant cpu
# Output: target\dist\dictate-v<version>-windows-x64-<variant>.zip plus a .sha256 next to each.
#
# Unlike .\build.ps1 (tuned for this PC), these builds must run on other machines:
#   - CPU code targets AVX2 instead of this CPU (GGML_NATIVE=OFF);
#   - CUDA kernels cover Turing to Blackwell (RTX 20 to RTX 50) plus PTX for newer GPUs;
#   - cuBLAS and the MSVC runtime DLLs are shipped next to the exe.
# Each variant builds in its own target folder, so the flags never mix with dev builds.
param([ValidateSet("all", "cuda", "cpu")][string]$Variant = "all")
$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$dist = Join-Path $root "target\dist"
New-Item -ItemType Directory -Force $dist | Out-Null

# portable CPU baseline for whisper.cpp / ggml (passed through to CMake by whisper-rs-sys)
$env:GGML_NATIVE = "OFF"
foreach ($f in "GGML_AVX", "GGML_AVX2", "GGML_FMA", "GGML_F16C", "GGML_BMI2") { Set-Item "env:$f" "ON" }

$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "MSVC redistributable folder not found in $vs" }

$variants = if ($Variant -eq "all") { @("cuda", "cpu") } else { @($Variant) }
foreach ($v in $variants) {
    Write-Host "== $v"
    $env:CARGO_TARGET_DIR = Join-Path $root "target\dist-build-$v"
    if ($v -eq "cuda") {
        $env:CMAKE_CUDA_ARCHITECTURES = "75-real;86-real;89-real;120-real;75-virtual"
        & (Join-Path $root "build.ps1") build
    } else {
        Remove-Item Env:CMAKE_CUDA_ARCHITECTURES -ErrorAction SilentlyContinue
        & (Join-Path $root "build.ps1") build --no-default-features
    }

    $name = "dictate-v$version-windows-x64-$v"
    $stage = Join-Path $dist $name
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage, (Join-Path $stage "models") | Out-Null
    foreach ($exe in "dictate.exe", "dictate-bench.exe") {
        Copy-Item (Join-Path $env:CARGO_TARGET_DIR "release\$exe") $stage
    }
    foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll") {
        Copy-Item (Join-Path $crt.FullName $dll) $stage
    }
    if ($v -eq "cuda") {
        $import = (Select-String -Path (Join-Path $stage "dictate.exe") -Pattern "cublas64_\d+\.dll" -List).Matches[0].Value
        $bin = @("bin\x64", "bin") | ForEach-Object { Join-Path $env:CUDA_PATH $_ } |
            Where-Object { Test-Path (Join-Path $_ $import) } | Select-Object -First 1
        foreach ($dll in $import, ($import -replace "cublas64", "cublasLt64")) { Copy-Item (Join-Path $bin $dll) $stage }
    }
    foreach ($f in "download-models.ps1", "download-models.cmd", "LICENSE") { Copy-Item (Join-Path $root $f) $stage }
    Copy-Item (Join-Path $root "dist\README.txt") $stage
    Copy-Item (Join-Path $root "dist\THIRD-PARTY-NOTICES.txt") $stage

    $zip = Join-Path $dist "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Push-Location $dist
    try { tar.exe -a -c -f "$name.zip" $name } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "zip failed" }
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    Set-Content -Path "$zip.sha256" -Value "$hash  $name.zip" -NoNewline -Encoding ascii
    Write-Host ("{0}  {1:N0} MB  {2}" -f $name, ((Get-Item $zip).Length / 1MB), $hash)
}
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
