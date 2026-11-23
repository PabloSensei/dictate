dictate: private, offline push-to-talk dictation (Linux beta, X11)
https://github.com/PabloSensei/dictate

This is a beta. It has a tray icon, the hotkey, typing and the bubble; the
settings window and the history window of the Windows version are not there yet:
settings live in a text file (tray menu > Settings opens it), and tray menu >
History lists recent dictations (click one to copy it).

X11 only for now. On Ubuntu, pick "Ubuntu on Xorg" with the gear icon on the login
screen. Check with:  echo $XDG_SESSION_TYPE   (it must say x11)

GETTING STARTED

1. Unpack this folder anywhere, for example to ~/.local/opt/dictate.

2. Download the speech model (0.5 GB):
       ./download-models.sh
   It goes to ~/.local/share/dictate/models and is checked by SHA-256.

3. Run ./dictate. A microphone icon appears in the tray (on GNOME it needs the
   AppIndicator extension, which Ubuntu has on by default).

4. Click into any text field, hold Right Ctrl, speak, and release.

   ./dictate --quit stops it, even without a tray icon.
   Tray menu > Start at login adds it to ~/.config/autostart.

SETTINGS

~/.local/share/dictate/dictate.ini, a plain text file with comments. After editing,
use tray menu > Restart. Tray menu > Microphone picks the microphone. Useful keys:
hotkey (RCtrl, RAlt, RShift, F9...), mode (hold or toggle), language (en, ru,
auto...), prompt (names and terms you say often), clipboard, history, overlay
(bottom, center, off), live_preview, check_updates, threads (set it to your
number of physical cores).

Speed: the "small" model on the CPU takes about 2-5 s per phrase on a recent
8-core CPU. "./download-models.sh '' turbo" gets large-v3-turbo: more accurate,
several times slower on a CPU.

REQUIREMENTS

x86_64 with AVX2, glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+, Fedora 36+...),
an X11 session with a compositor for the transparent bubble, ALSA/PulseAudio/
PipeWire for the microphone. notify-send (libnotify-bin) shows errors.

PRIVACY

Everything runs on your computer. There is no network code in the app; it never
sends audio or text anywhere. The log (dictate.log) has timings, never text.
The only request is the update check: at startup (and once a day) the system's
curl asks api.github.com for the newest version. check_updates = false in
dictate.ini turns it off.

UPDATES

When a newer version is out, a notification says so and the tray menu offers
"Update to ...": dictate downloads it, checks its SHA-256, replaces its files
(dictate.ini and the models stay) and restarts.
