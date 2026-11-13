# Downloads the default models from Hugging Face and checks their SHA-256:
#   ggml-large-v3-turbo.bin   Whisper large-v3-turbo, 1.6 GB  (ggerganov/whisper.cpp)
#   ggml-silero-v6.2.0.bin    Silero VAD, 0.9 MB              (ggml-org/whisper-vad)
# Run from PowerShell in this folder:
#   .\download-models.ps1                      into %LOCALAPPDATA%\Programs\dictate\models
#   .\download-models.ps1 -Dir D:\models       anywhere else (then point dictate.ini at it)
# Files that are already there with the right checksum are skipped.
param([string]$Dir = (Join-Path $env:LOCALAPPDATA "Programs\dictate\models"))
$ErrorActionPreference = "Stop"

$models = @(
    @{ Name = "ggml-large-v3-turbo.bin"
       Url  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
       Sha  = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69" },
    @{ Name = "ggml-silero-v6.2.0.bin"
       Url  = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin"
       Sha  = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987" }
)

New-Item -ItemType Directory -Force $Dir | Out-Null
foreach ($m in $models) {
    $path = Join-Path $Dir $m.Name
    if ((Test-Path $path) -and (Get-FileHash $path -Algorithm SHA256).Hash -eq $m.Sha) {
        Write-Host "ok        $($m.Name)"
        continue
    }
    Write-Host "download  $($m.Name)"
    $tmp = "$path.part"
    curl.exe -L --fail --retry 3 -C - -o $tmp $m.Url
    if ($LASTEXITCODE -ne 0) { throw "download failed: $($m.Url)" }
    $got = (Get-FileHash $tmp -Algorithm SHA256).Hash
    if ($got -ne $m.Sha) {
        Remove-Item $tmp
        throw "checksum mismatch for $($m.Name): got $got"
    }
    Move-Item $tmp $path -Force
    Write-Host "ok        $($m.Name)"
}
Write-Host "Models are in $Dir"
