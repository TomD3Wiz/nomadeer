#!/bin/sh
# Cross-builds both Windows editions (x86_64-pc-windows-gnu) and the portable
# zip inside a Debian-based Rust container. `make windows-installer` runs it as:
#
#   build-cross.sh <version>
#
# with the repository mounted read-only at /src, a writable build directory at
# /build and the output directory at /dist. The installer itself is compiled
# afterwards with Inno Setup from the binaries left in /build/stage.
set -eu

version=${1:?usage: build-cross.sh <version>}
src=${SRC_DIR:-/src}
build=${BUILD_DIR:-/build}
dist=${DIST_DIR:-/dist}
target=x86_64-pc-windows-gnu

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 zip >/dev/null

# Keep the registry cache between runs and stay on the image's pinned
# toolchain instead of letting rust-toolchain.toml download the newest stable.
export CARGO_HOME="$build/cargo-home"
RUSTUP_TOOLCHAIN=$(rustup default | cut -d' ' -f1)
export RUSTUP_TOOLCHAIN
rustup target add "$target"

cd "$src"
echo "== optimized edition (x86-64-v3)"
RUSTFLAGS="-C target-cpu=x86-64-v3" \
    cargo build --release --locked --target "$target" --target-dir "$build/optimized"
echo "== compatible edition (x86-64)"
cargo build --release --locked --target "$target" --target-dir "$build/compatible"

stage="$build/stage"
rm -rf "$stage"
mkdir -p "$stage"
cp "$build/optimized/$target/release/nomadeer.exe" "$stage/nomadeer-optimized.exe"
cp "$build/compatible/$target/release/nomadeer.exe" "$stage/nomadeer-compatible.exe"

# Text files in the zip get Windows line endings so Notepad shows them as-is.
crlf() {
    sed 's/\r$//; s/$/\r/' "$1"
}

echo "== portable zip"
name="Nomadeer-$version-Windows-Portable"
portable="$build/portable"
rm -rf "$portable"
mkdir -p "$portable/$name"
cp "$stage/nomadeer-optimized.exe" "$portable/$name/Nomadeer.exe"
cp "$stage/nomadeer-compatible.exe" "$portable/$name/Nomadeer-compatible.exe"
: >"$portable/$name/portable"
crlf packaging/windows/portable-README.txt | sed "s/@VERSION@/$version/g" >"$portable/$name/README.txt"
crlf LICENSE >"$portable/$name/LICENSE.txt"
rm -f "$dist/$name.zip" "$dist/$name.zip.sha256"
(cd "$portable" && zip -q -r -X "$dist/$name.zip" "$name")
(cd "$dist" && sha256sum "$name.zip" >"$name.zip.sha256")
echo "wrote $dist/$name.zip"
