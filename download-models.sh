#!/bin/sh
# Downloads the models dictate needs on macOS and Linux and checks their SHA-256:
#   ggml-small.bin           Whisper small, 0.5 GB          (ggerganov/whisper.cpp)
#     or ggml-large-v3-turbo.bin  1.6 GB, more accurate, slower on a CPU
#   ggml-silero-v6.2.0.bin   Silero VAD, 0.9 MB              (ggml-org/whisper-vad)
#
#   ./download-models.sh                 into the folder dictate reads them from
#   ./download-models.sh DIR             anywhere else (then point dictate.ini at it)
#   ./download-models.sh "" turbo        large-v3-turbo instead of small
#                                        (then set model and model_sha256 in dictate.ini)
# The default folder is ~/Library/Application Support/dictate/models on macOS and
# ${XDG_DATA_HOME:-~/.local/share}/dictate/models on Linux.
# Files that are already there with the right checksum are skipped.
set -eu

dir=${1:-}
model=${2:-small}
if [ -z "$dir" ]; then
    if [ "$(uname -s)" = "Darwin" ]; then
        dir="$HOME/Library/Application Support/dictate/models"
    else
        dir="${XDG_DATA_HOME:-$HOME/.local/share}/dictate/models"
    fi
fi

case "$model" in
    small) whisper=ggml-small.bin; whisper_sha=1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b ;;
    turbo) whisper=ggml-large-v3-turbo.bin; whisper_sha=1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69 ;;
    *) echo "unknown model '$model' (small or turbo)" >&2; exit 2 ;;
esac

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

fetch() { # name url sha
    path="$dir/$1"
    if [ -f "$path" ] && [ "$(sha256 "$path")" = "$3" ]; then
        echo "ok        $1"
        return
    fi
    echo "download  $1"
    curl -L --fail --retry 3 -C - -o "$path.part" "$2"
    got=$(sha256 "$path.part")
    if [ "$got" != "$3" ]; then
        rm -f "$path.part"
        echo "checksum mismatch for $1: got $got" >&2
        exit 1
    fi
    mv -f "$path.part" "$path"
    echo "ok        $1"
}

mkdir -p "$dir"
fetch "$whisper" "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$whisper" "$whisper_sha"
fetch ggml-silero-v6.2.0.bin https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin \
    2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987
echo "Models are in $dir"
