#!/bin/sh
# Builds the single-file bundle Nomadeer-<version>-x86_64.flatpak (plus .sha256)
# from the prebuilt release binaries in target/{optimized,compatible}/release.
#
# Usage: build-flatpak.sh VERSION OUT_DIR
#
# Needs flatpak and flatpak-builder with the org.freedesktop.Sdk named in the
# manifest; the ghcr.io/flathub-infra/flatpak-github-actions image has both
# (make flatpak runs it there). Build state goes to FLATPAK_WORK_DIR, default
# target/flatpak.
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 VERSION OUT_DIR" >&2
    exit 2
fi
version=$1
out=$2

APP_ID=io.github.TomD3Wiz.Nomadeer
repo=$(cd "$(dirname "$0")/../../.." && pwd)
manifest="$repo/packaging/linux/flatpak/$APP_ID.yml"
work=${FLATPAK_WORK_DIR:-$repo/target/flatpak}

mkdir -p "$work" "$out"
# flatpak-builder keeps state in the current directory unless told otherwise;
# working from $work leaves the source tree untouched (it may be read-only).
cd "$work"
# rofiles-fuse needs FUSE, which containers usually lack.
flatpak-builder --disable-rofiles-fuse --force-clean \
    --state-dir="$work/state" --repo="$work/repo" \
    "$work/build" "$manifest"

bundle="Nomadeer-$version-x86_64.flatpak"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$work/repo" "$out/$bundle" "$APP_ID"
(cd "$out" && sha256sum "$bundle" > "$bundle.sha256")
echo "Built $out/$bundle"
