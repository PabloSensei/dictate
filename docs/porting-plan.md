# macOS and Linux port: working plan (branch `port`)

Delete this file before merging into `main`.

## Decisions (from the user, 2026-11-17)

- **First milestone:** tray, hotkey, microphone, typing and bubble on macOS and Linux. Settings and
  history are edited in `dictate.ini`; the settings window and history UI come later.
- **Linux test target:** Ubuntu on Xorg (X11). Wayland is not supported yet.
- **macOS test machine:** Intel MacBook Pro 2019 only. Apple Silicon builds ship as untested.
- **Release:** a pre-release (v0.3.0-beta.N) with a test checklist. Windows stays as it is.

## Architecture

- `src/app.rs` (bin): platform-agnostic core. Shared state, engine and hotkey threads, history,
  restart handoff. UIs register a poster for `UiEvent`s.
- `src/win/`: tray, wndproc, overlay and settings window (unchanged behavior).
- `src/unix/`: shared by macOS and Linux: single-instance lock (`File::try_lock`), command line
  (`--quit`, `--version`), bubble animation state (`anim.rs`), model-missing hint.
- `src/linux/`: ksni tray, X11 ARGB override-redirect bubble with an empty input shape, a main
  loop on an mpsc channel.
- `src/mac/`: NSStatusItem with a menu rebuilt in `menuNeedsUpdate:`, NSPanel bubble (layer
  contents = CGImage), GCD main queue for core events, Accessibility/Input Monitoring prompts.
- Lib: `keys`, `inject`, `audio` (+ `audio_wavein.rs` / `audio_cpal.rs`), `clipboard_unix.rs`,
  `autostart_unix.rs`, `caption.rs` (fontdue), `icon.rs`, per-OS `log::data_dir` and config.

## Checks

- Windows: `.\build.ps1 clippy --all-targets`, `.\build.ps1 test`.
- Linux (WSL): `~/wsl-sync.sh cargo clippy --release --no-default-features --all-targets`,
  `tools/e2e-x11.sh` (Xvfb over TCP, xte for keys, xev as the typing target, fake mic from a WAV).
- macOS type check from WSL: `DOCS_RS=1 WHISPER_DONT_GENERATE_BINDINGS=1 cargo clippy --target
  x86_64-apple-darwin ...` (skips the whisper.cpp build).
- CI (`.github/workflows/build.yml`): Linux on ubuntu-22.04 with the Xvfb e2e test; macOS on
  macos-15-intel and macos-15 with a transcription check and a launch check.

## Status

- [x] .cargo/config.toml env moved to build.ps1
- [x] app.rs core extracted; Windows builds and passes tests (runtime not re-checked yet)
- [x] lib backends for Linux and macOS
- [x] linux UI; e2e on Xvfb passes in WSL (Cyrillic typed into xev)
- [x] macOS UI; type-checks and lints from WSL
- [ ] CI green on all three runners
- [x] download-models.sh
- [ ] pre-release with a test checklist for the user
- [ ] Windows runtime check of the refactored build before merging to main
