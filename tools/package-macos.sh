#!/bin/bash
# Builds Dictate.app and a zip for this Mac's architecture (run on macOS):
#   tools/package-macos.sh            -> dist-out/dictate-<version>-macos-<arch>.zip
# Apple Silicon gets the Metal build; Intel runs on the CPU with portable x86 flags.
# The app is signed ad hoc (no Apple Developer ID), so macOS asks to confirm the first
# launch, and permissions are granted again after each update.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
export MACOSX_DEPLOYMENT_TARGET=11.0
export CMAKE_OSX_DEPLOYMENT_TARGET=11.0
features=()
if [ "$arch" = arm64 ]; then
    features=(--features metal)
else
    export GGML_NATIVE=OFF GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON
fi
cargo build --release --no-default-features ${features[@]+"${features[@]}"}

out=dist-out
app="$out/Dictate.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/dictate "$app/Contents/MacOS/dictate"
sed "s/@VERSION@/$version/g" assets/Info.plist >"$app/Contents/Info.plist"
cp download-models.sh "$app/Contents/Resources/"
chmod +x "$app/Contents/Resources/download-models.sh"
cp dist/README-macos.txt "$app/Contents/Resources/README.txt"
cp dist/THIRD-PARTY-NOTICES.txt LICENSE "$app/Contents/Resources/"

# icon: every size iconutil wants, from the 256 px PNG
iconset=$(mktemp -d)/dictate.iconset
mkdir -p "$iconset"
for s in 16 32 128 256; do
    sips -z $s $s assets/dictate-256.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    [ $d -le 256 ] && sips -z $d $d assets/dictate-256.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/dictate.icns"

codesign --force --deep --sign - "$app"
codesign --verify --verbose "$app"
zip="$out/dictate-$version-macos-$arch.zip"
rm -f "$zip"
(cd "$out" && ditto -c -k --keepParent Dictate.app "$(basename "$zip")")
shasum -a 256 "$zip" | sed 's|  .*/|  |' >"$zip.sha256"
cp target/release/dictate-bench "$out/dictate-bench-macos-$arch"
echo "$zip"
