#!/bin/sh
# Starts the Nomadeer build that suits this processor. The AppImage (as AppRun)
# and the Flatpak install this as bin/nomadeer, with both builds next to it in
# lib/nomadeer/:
#   nomadeer-optimized   x86-64-v3 (AVX2/FMA); Umbra briefings run 2-3x faster
#   nomadeer-compatible  baseline x86-64 for older processors
# NOMADEER_EDITION=optimized|compatible skips the check.
set -eu

here=$(dirname "$(readlink -f "$0")")
lib="$here/../lib/nomadeer"

# Every x86-64-v3 feature, as /proc/cpuinfo names them (LZCNT shows as abm).
# Checking the full set keeps virtual machines that hide some of them on the
# compatible build.
supports_optimized() {
    flags=$(grep -m 1 '^flags' "${NOMADEER_CPUINFO:-/proc/cpuinfo}" 2>/dev/null) || return 1
    flags=" ${flags#*:} "
    for flag in avx avx2 bmi1 bmi2 f16c fma movbe abm; do
        case "$flags" in
            *" $flag "*) ;;
            *) return 1 ;;
        esac
    done
}

edition=${NOMADEER_EDITION:-}
case "$edition" in
    optimized | compatible) ;;
    "")
        if supports_optimized; then
            edition=optimized
        else
            edition=compatible
        fi
        ;;
    *)
        echo "NOMADEER_EDITION must be optimized or compatible, not '$edition'" >&2
        exit 2
        ;;
esac

exec "$lib/nomadeer-$edition" "$@"
