# Builds dictate. Run from PowerShell in this folder:
#   .\build.ps1                      release build of everything (CUDA)
#   .\build.ps1 test                 unit tests
#   .\build.ps1 build --bin dictate  any cargo subcommand + args (always --release)
#   .\build.ps1 build --no-default-features   CPU-only build, no CUDA Toolkit needed
# Imports the MSVC developer environment (cl, link, cmake, ninja), finds libclang
# for bindgen, and runs cargo.
$ErrorActionPreference = "Stop"

$cmd = "build"
$rest = @($args)
if ($rest.Count -gt 0 -and $rest[0] -in "build", "test", "check", "clippy", "run") {
    $cmd = $rest[0]
    $rest = @($rest | Select-Object -Skip 1)
}
$cuda = -not ($rest -contains "--no-default-features")

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "Visual Studio Build Tools not found (install the 'Desktop development with C++' workload)" }
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "Visual Studio C++ build tools not found" }
$vcvars = Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat"

$env:PATH = "$(Split-Path $vswhere);$env:PATH"
cmd /c "`"$vcvars`" >nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}
$tools = @("cmake", "ninja", "cl")
if ($cuda) { $tools += "nvcc" }
foreach ($tool in $tools) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool not found in PATH after vcvars64" }
}

# bindgen needs libclang.dll: LIBCLANG_PATH if set, else LLVM or the clang that ships with VS.
if (-not $env:LIBCLANG_PATH) {
    $candidates = @(
        (Join-Path $env:ProgramFiles "LLVM\bin"),
        (Join-Path $vs "VC\Tools\Llvm\x64\bin")
    )
    $found = $candidates | Where-Object { Test-Path (Join-Path $_ "libclang.dll") } | Select-Object -First 1
    if (-not $found) {
        throw "libclang.dll not found. Install LLVM (winget install LLVM.LLVM) or set `$env:LIBCLANG_PATH to its bin folder."
    }
    $env:LIBCLANG_PATH = $found
}

Push-Location $PSScriptRoot
try {
    cargo $cmd --release @rest
    if ($LASTEXITCODE -ne 0) { throw "cargo $cmd failed ($LASTEXITCODE)" }
} finally { Pop-Location }
