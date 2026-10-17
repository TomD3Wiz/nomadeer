#!/bin/sh
# Render assets/icon.svg to the PNG sizes and the Windows .ico used by
# packaging and the app window. Needs rsvg-convert (librsvg) and python3 with
# Pillow. Without them installed locally, run it in a container, e.g.:
#
#   podman run --rm --security-opt label=disable \
#     -v "$PWD:/src" -w /src docker.io/library/debian:12-slim \
#     sh -c 'apt-get update -qq && apt-get install -y -qq librsvg2-bin python3-pil >/dev/null \
#            && packaging/icons/render-icons.sh'
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
svg="$root/assets/icon.svg"
out="${1:-$root/assets/icons}"

mkdir -p "$out"
for size in 16 32 48 64 128 256 512; do
	rsvg-convert --width "$size" --height "$size" "$svg" -o "$out/nomadeer-$size.png"
done

python3 - "$out" <<'PY'
import sys
from pathlib import Path
from PIL import Image

out = Path(sys.argv[1])
sizes = [16, 32, 48, 64, 128, 256]
frames = [Image.open(out / f"nomadeer-{size}.png").convert("RGBA") for size in sizes]
# Each size comes from its own render rather than a downscale of the largest.
frames[-1].save(
    out / "nomadeer.ico",
    format="ICO",
    sizes=[(size, size) for size in sizes],
    append_images=frames[:-1],
)
PY
