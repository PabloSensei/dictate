# macOS and Linux port: working plan (branch `port`)

Delete this file before merging into `main`.

## Decisions (from the user, 2026-11-17)

- **First milestone:** tray, hotkey, microphone, typing and bubble on macOS and Linux. Settings and
  history are edited in `dictate.ini`; the settings window and history UI come later.
- **Linux test target:** Ubuntu on Xorg (X11). Wayland is not supported yet.
- **macOS test machine:** Intel MacBook Pro 2019 only. Apple Silicon builds ship as untested.
- **Release:** a pre-release (v0.3.0-beta.N) with a test checklist. Windows stays as it is.

## Architecture

- `src/app.rs` (bin): platform-agnostic core. Shared state, engine thread, hotkey thread (Trigger),
  history, restart handoff. It talks to the UI through `app::post(UiEvent)`, a function each
  platform registers.
- `src/win/` (bin, `cfg(windows)`): the existing tray, wndproc, overlay and settings window.
- `src/mac/` (bin, `cfg(target_os = "macos")`): NSApplication + NSStatusItem menu, overlay NSPanel
  (CALayer contents = CGImage of the bubble buffer), and an NSTimer that drains UI events.
- `src/linux/` (bin, `cfg(target_os = "linux")`): ksni tray (D-Bus StatusNotifierItem, optional),
  and an X11 overlay with an ARGB visual, override-redirect and an empty input shape.
  The main loop polls the UI channel at 60 Hz.
- Lib, per platform behind `cfg`:
  - `audio`: waveIn (Windows), cpal plus resampling to 16 kHz (others).
  - `keys`: GetAsyncKeyState (Windows), CGEventSource flags and key state (macOS),
    XQueryKeymap (X11).
  - `inject`: SendInput (Windows), CGEventKeyboardSetUnicodeString (macOS), XTest with a
    temporarily remapped keycode (X11).
  - `clipboard`: Win32 (Windows), arboard (others).
  - `autostart`: HKCU Run (Windows), LaunchAgent plist (macOS), `~/.config/autostart` (Linux).
  - `caption`: GDI (`surface.rs`, Windows), fontdue with a system font (others).
  - `log::data_dir`: `%LOCALAPPDATA%\dictate`, `~/Library/Application Support/dictate`,
    `$XDG_DATA_HOME/dictate`. On macOS and Linux, `dictate.ini` and `models/` live there too.
- Hotkeys keep Windows VK codes as the canonical id; each platform maps them.
  The macOS default is Right Option (MacBooks have no Right Ctrl); Linux uses Right Ctrl.

## Builds

- Windows: `build.ps1` / `package.ps1`, unchanged. CMAKE_GENERATOR now comes from `build.ps1`,
  not `.cargo/config.toml`.
- Non-Windows: always `--no-default-features` (no CUDA). macOS uses `--features metal` on arm64
  only; Intel Macs run on the CPU with the small model.
- Portable CPU flags in CI: `GGML_NATIVE=OFF GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON`
  on x86_64.
- Linux release builds come from CI on the oldest Ubuntu runner (glibc). The WSL Ubuntu 26.04 build
  is for development only.
- Dev loop in WSL:
  `cd ~/dictate && cargo build --release --no-default-features`
  with `LIBCLANG_PATH=/usr/lib/llvm-21/lib`. WSLg is not Xorg, so the real test is the user's Ubuntu.

## Status

- [ ] .cargo/config.toml env moved to build.ps1
- [ ] app.rs core extracted; Windows still builds, tests, runs (demo screenshots)
- [ ] lib: audio (cpal), keys, inject, clipboard, autostart, caption, data_dir for Linux
- [ ] linux UI: tray (optional), overlay, main loop; builds and runs in WSL
- [ ] macOS: same, compiled in CI
- [ ] CI workflow: macOS x86_64 + arm64 (.app zip), Linux x86_64 (tar.gz); smoke tests
- [ ] download-models.sh
- [ ] pre-release with a test checklist for the user
