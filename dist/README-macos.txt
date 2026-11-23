dictate: private, offline push-to-talk dictation (macOS beta)
https://github.com/PabloSensei/dictate

This is a beta. It has a menu bar icon, the hotkey, typing and the bubble; the
settings window and the history window of the Windows version are not there yet:
settings live in a text file (menu > Settings opens it), and menu > History
lists recent dictations (click one to copy it).

GETTING STARTED

1. Unzip and move Dictate.app to /Applications. Run it from there, not from
   Downloads: macOS runs apps from Downloads in a temporary location, and the
   permissions and "start at login" would point to the wrong place.

2. The app is not notarized by Apple, so the first launch is blocked. Either
   right-click Dictate.app > Open > Open, or, if macOS only offers "Move to Trash",
   open System Settings > Privacy & Security, scroll down and click "Open Anyway".
   From Terminal the same thing is:
       xattr -dr com.apple.quarantine /Applications/Dictate.app

3. Download the speech model (0.5 GB) in Terminal:
       /Applications/Dictate.app/Contents/Resources/download-models.sh
   It goes to ~/Library/Application Support/dictate/models and is checked by SHA-256.

4. Start Dictate. macOS asks for three permissions; allow them in System Settings >
   Privacy & Security, then use menu > Restart:
     - Accessibility: to type the text into other apps
     - Input Monitoring: to see the hotkey while other apps are in front
     - Microphone: asked on the first dictation (that first one may come out empty)

5. Click into any text field, hold Right Option, speak, and release.

SETTINGS

~/Library/Application Support/dictate/dictate.ini, a plain text file with comments
(menu > Settings opens it in TextEdit). After editing, use menu > Restart.
Menu > Microphone picks the microphone. Useful keys: hotkey (ROption, RCmd, RShift,
F13...), mode (hold or toggle), language (en, ru, auto...), prompt (names and
terms you say often), clipboard, history, overlay (bottom, center, off),
live_preview, check_updates.

Apple Silicon builds use the GPU (Metal). Intel Macs run on the CPU with the
"small" model: about 2-5 s per phrase on a recent 6-8 core CPU. On Apple Silicon
you can switch to the more accurate large-v3-turbo:
    download-models.sh "" turbo
and set model / model_sha256 in dictate.ini as that script tells you.

PRIVACY

Everything runs on your Mac. There is no network code in the app; it never
sends audio or text anywhere. The log (dictate.log) has timings, never text.
The only request is the update check: at startup (and once a day) the system's
curl asks api.github.com for the newest version. check_updates = false in
dictate.ini turns it off.

UPDATES

When a newer version is out, a dialog offers to update (also menu > Update to ...):
dictate downloads it, checks its SHA-256, replaces Dictate.app (dictate.ini and
the models stay) and restarts. The app is signed ad hoc, not by an Apple
developer ID, so macOS may ask for Accessibility and Input Monitoring again.

UNINSTALL

Turn off menu > Start at login, quit, then delete Dictate.app and
~/Library/Application Support/dictate.
