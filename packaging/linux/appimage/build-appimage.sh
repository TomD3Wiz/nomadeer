#!/bin/sh
# Builds Nomadeer-<version>-x86_64.AppImage (plus .sha256 and .zsync) holding
# both release builds. AppRun is the launcher, which picks the optimized build
# on processors with AVX2/FMA and the compatible one otherwise.
#
# Usage: build-appimage.sh VERSION OPTIMIZED_BIN COMPATIBLE_BIN OUT_DIR
#
# Needs curl and sha256sum; appimagetool and the AppImage runtime are fetched
# (pinned and checksummed) into APPIMAGE_WORK_DIR, default target/appimage.
# appimagetool runs with APPIMAGE_EXTRACT_AND_RUN=1, so FUSE is not needed.
# Set APPIMAGE_MAX_GLIBC (e.g. 2.35) to fail when a build needs a newer glibc.
#
# The AppImage carries update information pointing at the newest GitHub
# release's .zsync file, so AppImage updaters can fetch only what changed.
# appimagetool writes the .zsync when zsyncmake (package zsync) is installed;
# without it the .zsync is skipped with a warning, or the build fails when
# APPIMAGE_REQUIRE_ZSYNC=1 (as the release workflow sets).
set -eu

if [ $# -ne 4 ]; then
    echo "usage: $0 VERSION OPTIMIZED_BIN COMPATIBLE_BIN OUT_DIR" >&2
    exit 2
fi
version=$1
optimized=$2
compatible=$3
out=$4

APP_ID=io.github.TomD3Wiz.Nomadeer
APPIMAGETOOL_VERSION=1.9.1
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
RUNTIME_VERSION=20251108
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
# Must match the release file names: Nomadeer-<version>-x86_64.AppImage.zsync.
UPDATE_INFORMATION="gh-releases-zsync|TomD3Wiz|nomadeer|latest|Nomadeer-*-x86_64.AppImage.zsync"

repo=$(cd "$(dirname "$0")/../../.." && pwd)
work=${APPIMAGE_WORK_DIR:-$repo/target/appimage}

# Downloads $1 to $3 unless $3 already matches sha256 $2.
fetch() {
    if ! echo "$2  $3" | sha256sum -c --status 2>/dev/null; then
        curl -fsSL --retry 3 -o "$3.part" "$1"
        echo "$2  $3.part" | sha256sum -c --quiet
        mv "$3.part" "$3"
    fi
}

# Highest glibc symbol version a binary cannot start without (weak symbols are
# optional and skipped). Empty when objdump is missing.
glibc_needed() {
    command -v objdump >/dev/null 2>&1 || return 0
    objdump -T "$1" | awk '$2 != "w"' | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1
}

if [ "${APPIMAGE_REQUIRE_ZSYNC:-0}" = 1 ] && ! command -v zsyncmake >/dev/null 2>&1; then
    echo "APPIMAGE_REQUIRE_ZSYNC=1 is set, but the .zsync file needs zsyncmake (zsync)" >&2
    exit 1
fi
if [ -n "${APPIMAGE_MAX_GLIBC:-}" ] && ! command -v objdump >/dev/null 2>&1; then
    echo "APPIMAGE_MAX_GLIBC is set, but checking it needs objdump (binutils)" >&2
    exit 1
fi
for bin in "$optimized" "$compatible"; do
    [ -f "$bin" ] || { echo "missing release build: $bin" >&2; exit 1; }
    need=$(glibc_needed "$bin")
    [ -n "$need" ] || continue
    echo "$bin needs glibc $need or newer"
    if [ -n "${APPIMAGE_MAX_GLIBC:-}" ] &&
        [ "$(printf '%s\n%s\n' "$need" "$APPIMAGE_MAX_GLIBC" | sort -V | tail -n 1)" != "$APPIMAGE_MAX_GLIBC" ]; then
        echo "$bin needs glibc $need, newer than APPIMAGE_MAX_GLIBC=$APPIMAGE_MAX_GLIBC" >&2
        exit 1
    fi
done

mkdir -p "$work/tools" "$out"
# Absolute, as appimagetool runs from $out below.
work=$(cd "$work" && pwd)
out=$(cd "$out" && pwd)
tools="$work/tools"
appdir="$work/Nomadeer.AppDir"
appimagetool="$tools/appimagetool-$APPIMAGETOOL_VERSION-x86_64.AppImage"
runtime="$tools/runtime-$RUNTIME_VERSION-x86_64"
fetch "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-x86_64.AppImage" \
    "$APPIMAGETOOL_SHA256" "$appimagetool"
fetch "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-x86_64" \
    "$RUNTIME_SHA256" "$runtime"
chmod +x "$appimagetool"

rm -rf "$appdir"
install -Dm755 "$repo/packaging/linux/nomadeer-launcher.sh" "$appdir/usr/bin/nomadeer"
install -Dm755 "$optimized" "$appdir/usr/lib/nomadeer/nomadeer-optimized"
install -Dm755 "$compatible" "$appdir/usr/lib/nomadeer/nomadeer-compatible"
install -Dm644 "$repo/packaging/linux/$APP_ID.desktop" "$appdir/usr/share/applications/$APP_ID.desktop"
# appimagetool only checks metainfo under the legacy .appdata.xml name.
install -Dm644 "$repo/packaging/linux/$APP_ID.metainfo.xml" "$appdir/usr/share/metainfo/$APP_ID.appdata.xml"
for size in 16 32 48 64 128 256 512; do
    install -Dm644 "$repo/assets/icons/nomadeer-$size.png" \
        "$appdir/usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
install -Dm644 "$repo/assets/icon.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "$repo/LICENSE" "$appdir/usr/share/doc/nomadeer/LICENSE"

# The AppDir root carries the entry points appimagetool and desktop
# integration tools look for.
ln -s usr/bin/nomadeer "$appdir/AppRun"
cp "$appdir/usr/share/applications/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
cp "$repo/assets/icons/nomadeer-256.png" "$appdir/$APP_ID.png"
ln -s "$APP_ID.png" "$appdir/.DirIcon"

# Validate the metainfo offline here and stop appimagetool doing it online: its
# check fetches screenshot URLs, which point at main and do not exist until the
# release branch is merged.
if command -v appstreamcli >/dev/null 2>&1; then
    appstreamcli validate --no-net "$appdir/usr/share/metainfo/$APP_ID.appdata.xml"
elif [ "${APPIMAGE_REQUIRE_APPSTREAM:-0}" = 1 ]; then
    echo "appstreamcli is not installed, so the metainfo cannot be validated" >&2
    exit 1
else
    echo "warning: appstreamcli is not installed, so the metainfo was not validated" >&2
fi

target="$out/Nomadeer-$version-x86_64.AppImage"
rm -f "$target" "$target.sha256" "$target.zsync"
# appimagetool writes the .zsync into the current directory.
(cd "$out" && ARCH=x86_64 VERSION=$version APPIMAGE_EXTRACT_AND_RUN=1 \
    "$appimagetool" --no-appstream --runtime-file "$runtime" --mksquashfs-opt -no-xattrs \
    --updateinformation "$UPDATE_INFORMATION" "$appdir" "$target")
(cd "$out" && sha256sum "$(basename "$target")" > "$(basename "$target").sha256")
if [ -f "$target.zsync" ]; then
    echo "Built $target.zsync"
elif [ "${APPIMAGE_REQUIRE_ZSYNC:-0}" = 1 ]; then
    echo "appimagetool did not write $target.zsync" >&2
    exit 1
else
    echo "warning: zsyncmake is not installed, so $target.zsync was not built" >&2
fi
echo "Built $target"
