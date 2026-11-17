#!/bin/bash
# Builds the Linux tarball (run on the oldest Ubuntu you want to support: glibc):
#   tools/package-linux.sh            -> dist-out/dictate-<version>-linux-x86_64.tar.gz
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
export GGML_NATIVE=OFF GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON
cargo build --release --no-default-features

out=dist-out
name="dictate-$version-linux-x86_64"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
cp target/release/dictate download-models.sh "$stage/"
chmod +x "$stage/dictate" "$stage/download-models.sh"
cp dist/README-linux.txt "$stage/README.txt"
cp dist/THIRD-PARTY-NOTICES.txt LICENSE "$stage/"
cp assets/dictate-256.png "$stage/dictate.png"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
cp target/release/dictate-bench "$out/dictate-bench-linux-x86_64"
echo "$out/$name.tar.gz"
