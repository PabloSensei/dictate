#!/bin/bash
# End-to-end test of the Linux app on a virtual X server, no microphone needed:
# holds the hotkey with XTest while DICTATE_FAKE_MIC feeds a recording, then checks
# that the recognized text was typed into a test window (xev) and that the bubble
# showed up. Needs: Xvfb, xdotool, x11-utils (xev, xwininfo), imagemagick (import).
#
#   tools/e2e-x11.sh <path/to/dictate> <speech.wav (16 kHz mono)> [out-dir]
#
# Runs with its own data folder and dictate.ini (hotkey F9: xdotool adds Left Ctrl to a
# Right Ctrl press, which the app rightly treats as a shortcut). Models come from
# $MODELS (default: ~/.local/share/dictate/models, where download-models.sh puts them).
# Screenshots and logs go to out-dir (default ./e2e-out).
set -u
bin=$(realpath "$1")
wav=$(realpath "$2")
out=$(realpath -m "${3:-e2e-out}")
mkdir -p "$out"
models=${MODELS:-$HOME/.local/share/dictate/models}
export XDG_DATA_HOME=$(mktemp -d)
data="$XDG_DATA_HOME/dictate"
mkdir -p "$data"
ln -s "$(realpath "$models")" "$data/models"
cat >"$data/dictate.ini" <<INI
model = models/ggml-small.bin
vad_model = models/ggml-silero-v6.2.0.bin
hotkey = F9
history = off
INI
log="$data/dictate.log"
fail() { echo "FAIL: $*"; cleanup; exit 1; }
cleanup() {
    [ -n "${APP:-}" ] && kill "$APP" 2>/dev/null
    [ -n "${XEV:-}" ] && kill "$XEV" 2>/dev/null
    [ -n "${XVFB:-}" ] && kill "$XVFB" 2>/dev/null
    cp -f "$log" "$out/dictate.log" 2>/dev/null
}

# TCP, because some setups (WSLg) don't let Xvfb create its socket in /tmp/.X11-unix
export DISPLAY=127.0.0.1:99
Xvfb :99 -listen tcp -ac -screen 0 1600x900x24 +extension COMPOSITE >"$out/xvfb.log" 2>&1 &
XVFB=$!
for _ in $(seq 40); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.25; done
xdpyinfo >/dev/null 2>&1 || fail "Xvfb did not start: $(tail -5 "$out/xvfb.log")"
xev -event keyboard -geometry 500x200+50+50 -name dictate-target >"$out/xev.log" 2>&1 &
XEV=$!
target=
for _ in $(seq 40); do
    target=$(xdotool search --name dictate-target 2>/dev/null | head -1)
    [ -n "$target" ] && break
    sleep 0.25
done
[ -n "$target" ] || fail "no xev window: $(cat "$out/xev.log")"
xdotool windowfocus "$target"

: >"$log"
DICTATE_FAKE_MIC="$wav" "$bin" >"$out/stdout.log" 2>&1 &
APP=$!
for _ in $(seq 120); do
    grep -q "model loaded" "$log" 2>/dev/null && break
    kill -0 "$APP" 2>/dev/null || fail "dictate exited: $(cat "$out/stdout.log")"
    sleep 0.5
done
grep -q "model loaded" "$log" || fail "model did not load: $(tail -3 "$log")"

secs=$(python3 -c "import wave,sys; w=wave.open(sys.argv[1]); print(w.getnframes()/w.getframerate())" "$wav")
xte "keydown F9"
sleep 1.5
import -window root "$out/bubble.png"
xwininfo -root -tree | grep -q "dictate bubble" && echo "bubble window: present" || echo "bubble window: MISSING"
sleep "$(python3 -c "print(max(0.5, $secs - 1.0))")"
xte "keyup F9"
for _ in $(seq 120); do
    grep -q "^[0-9]* dictation\|no speech\|error:" "$log" && break
    sleep 0.5
done
sleep 2
typed=$(grep -o 'XLookupString gives [0-9]* bytes: ([0-9a-f ]*) "[^"]*"' "$out/xev.log" | sed 's/.*"\(.*\)"/\1/' |
    awk 'NR % 2 == 1' | tr -d '\n')
echo "log:   $(grep -E 'dictation|no speech|error' "$log" | tail -2 | tr '\n' ' ')"
echo "typed: $typed"
kill -0 "$APP" 2>/dev/null || fail "dictate crashed"
cleanup
[ -n "$typed" ] || { echo "FAIL: nothing was typed"; exit 1; }
echo "PASS"
