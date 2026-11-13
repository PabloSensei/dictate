# Downloads the default models from Hugging Face and checks their SHA-256:
#   ggml-large-v3-turbo.bin   Whisper large-v3-turbo, 1.6 GB  (ggerganov/whisper.cpp)
#     or ggml-small.bin       Whisper small, 0.5 GB, for the CPU-only build
#   ggml-silero-v6.2.0.bin    Silero VAD, 0.9 MB              (ggml-org/whisper-vad)
# Double-click download-models.cmd, or run from PowerShell in this folder:
#   .\download-models.ps1                      into the models folder of this copy (see below)
#   .\download-models.ps1 -Dir D:\models       anywhere else (then point dictate.ini at it)
# Next to dictate.exe (the release zip) the default is .\models; in the source tree it is
# %LOCALAPPDATA%\Programs\dictate\models, where install.ps1 puts the app.
# Files that are already there with the right checksum are skipped.
# The Whisper model matches the build: large-v3-turbo for CUDA, small (0.5 GB) for the
# CPU-only zip, which is the one without cuBLAS DLLs. -Model turbo|small picks it yourself.
param([string]$Dir, [ValidateSet("", "turbo", "small")][string]$Model = "")
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$portable = Test-Path (Join-Path $here "dictate.exe")
if (-not $Dir) {
    $Dir = if ($portable) { Join-Path $here "models" } else { Join-Path $env:LOCALAPPDATA "Programs\dictate\models" }
}
if (-not $Model) {
    $cpuOnly = $portable -and -not (Get-ChildItem $here -Filter "cublas64_*.dll" -ErrorAction SilentlyContinue)
    $Model = if ($cpuOnly) { "small" } else { "turbo" }
}

$whisper = @{
    turbo = @{ Name = "ggml-large-v3-turbo.bin"; Sha = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69" }
    small = @{ Name = "ggml-small.bin"; Sha = "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b" }
}[$Model]
$models = @(
    @{ Name = $whisper.Name
       Url  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$($whisper.Name)"
       Sha  = $whisper.Sha },
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
