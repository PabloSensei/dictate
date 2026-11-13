# Installs the release build to %LOCALAPPDATA%\Programs\dictate and starts it.
# Run from PowerShell in this folder after .\build.ps1:
#   .\install.ps1
# An existing dictate.ini and models folder there are kept as is. If the CUDA
# Toolkit isn't in PATH, the cuBLAS DLLs the app needs are copied next to it.
$ErrorActionPreference = "Stop"
$dst = Join-Path $env:LOCALAPPDATA "Programs\dictate"
New-Item -ItemType Directory -Force $dst, (Join-Path $dst "models") | Out-Null

Get-Process dictate -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dst\*" } | Stop-Process -Force
Start-Sleep -Milliseconds 500

foreach ($exe in "dictate.exe", "dictate-bench.exe") {
    Copy-Item (Join-Path $PSScriptRoot "target\release\$exe") $dst -Force
}

# CUDA builds link cuBLAS dynamically (cublasLt is loaded by cublas). The version
# suffix (_12, _13) comes from the CUDA Toolkit the exe was built with.
$import = Select-String -Path (Join-Path $dst "dictate.exe") -Pattern "cublas64_\d+\.dll" -List
if ($import) {
    $cublas = $import.Matches[0].Value
    if (-not (Get-Command $cublas -ErrorAction SilentlyContinue) -and $env:CUDA_PATH) {
        $bin = @("bin\x64", "bin") | ForEach-Object { Join-Path $env:CUDA_PATH $_ } |
            Where-Object { Test-Path (Join-Path $_ $cublas) } | Select-Object -First 1
        if ($bin) {
            foreach ($dll in $cublas, ($cublas -replace "cublas64", "cublasLt64")) { Copy-Item (Join-Path $bin $dll) $dst -Force }
            Write-Host "Copied cuBLAS DLLs from $bin"
        }
    }
}

if (-not (Get-ChildItem (Join-Path $dst "models") -Filter *.bin -ErrorAction SilentlyContinue)) {
    Write-Host "No models in $dst\models. Unless dictate.ini points elsewhere, run:  .\download-models.ps1"
}
Start-Process (Join-Path $dst "dictate.exe")
Write-Host "Installed to $dst"
