dictate: private, offline push-to-talk dictation for Windows
https://github.com/PabloSensei/dictate

GETTING STARTED

1. Unpack this folder anywhere, for example to %LOCALAPPDATA%\Programs\dictate.
   Settings (dictate.ini) and models are kept inside it, so it is fully portable.

2. Double-click download-models.cmd. It downloads the Whisper model (large-v3-turbo,
   1.6 GB, in the cuda zip; small, 0.5 GB, in the cpu zip) and the Silero voice
   activity detector from Hugging Face into the models folder and checks their SHA-256.

3. Run dictate.exe. Windows SmartScreen may warn about an unknown publisher
   because the exe is not code-signed: click "More info", then "Run anyway".

4. Click into any text field, hold Right Ctrl, speak, and release.

Left-click the tray icon to open the settings: hotkey, hold or toggle mode,
speech language, vocabulary hint, bubble, copy to clipboard, history, interface
language, start with Windows. Tray menu > History... shows what you dictated.

REQUIREMENTS

  cuda zip: Windows 10/11 x64, an NVIDIA GeForce RTX 20 series or newer
            (Turing and later), driver 580 or newer, about 2 GB of free VRAM.
  cpu zip:  Windows 10/11 x64 and a CPU with AVX2, no GPU needed. It uses the
            smaller Whisper "small" model with 8 threads: about 2-5 s per phrase
            on a modern 8-core desktop CPU, and somewhat less accurate than
            large-v3-turbo. Set "threads" in dictate.ini to your number of
            performance cores. large-v3-turbo also works on a CPU but takes
            about 12 s per phrase (download-models.ps1 -Model turbo).

PRIVACY

dictate has no network code. Audio stays in memory and text is typed with
SendInput. The clipboard is used only if you turn on "Copy to clipboard" or press
Copy in the history. The history is kept in memory until you quit, unless you
choose "On disk". The log (%LOCALAPPDATA%\dictate\dictate.log) holds only
timings and errors. The only download is download-models.cmd, which
you run yourself.

FILES

  dictate.exe              the app (tray)
  dictate-bench.exe        compare models on your own voice (see the GitHub README)
  download-models.cmd/.ps1 model downloader
  *.dll                    cuBLAS (cuda zip) and the Microsoft C++ runtime
  LICENSE, THIRD-PARTY-NOTICES.txt
