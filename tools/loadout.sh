#!/usr/bin/env bash
# Make a loadout variant of a squad bot: the same bot with one unit slot's primary weapon forced, for a
# loadout A/B against its base (bots/README.md, "Loadout A/B").
#   tools/loadout.sh <base> <slot> <arm>        e.g. tools/loadout.sh saver 3 Marksman
# <base> is a squad bot under bots/ (sapper, retaker, lurker, saver, forcer), <slot> a unit slot 0 to 4 and
# <arm> a `squad::Arm` variant (Rifle, Marksman, MachineGun, Shotgun, Smg). Writes the crate to
# out/loadout/<base>-<arm>-s<slot>/ (ignored by git) and prints its path; pass that path to ibc-bench.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -eq 3 ] || { sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
base=$1 slot=$2 arm=$3
[[ "$base" =~ ^[a-z]+$ ]] || { echo "base must be a bot name under bots/" >&2; exit 1; }
[[ "$slot" =~ ^[0-4]$ ]] || { echo "slot must be 0 to 4" >&2; exit 1; }
[[ "$arm" =~ ^[A-Z][A-Za-z]*$ ]] && grep -qE "^    $arm,\$" bots/squad/src/buy.rs ||
    { echo "unknown arm '$arm' (see squad::Arm in bots/squad/src/buy.rs)" >&2; exit 1; }
src="bots/$base"
grep -q 'loadout: None' "$src/src/lib.rs" 2>/dev/null || { echo "bots/$base is not a squad bot with a STYLE" >&2; exit 1; }

name="$base-$(echo "$arm" | tr '[:upper:]' '[:lower:]')-s$slot"
crate="${name//-/_}"
dst="out/loadout/$name"
rm -rf "$dst"
mkdir -p "$dst/src"
[ -f "$src/Cargo.lock" ] && cp "$src/Cargo.lock" "$dst/Cargo.lock"
# One directory deeper than bots/<base>, and squad is no longer a sibling.
sed -e "s|^name = \"$base\"|name = \"$crate\"|" \
    -e 's|path = "\.\./\.\./|path = "../../../|' \
    -e 's|path = "\.\./squad"|path = "../../../bots/squad"|' \
    "$src/Cargo.toml" >"$dst/Cargo.toml"
sed -e "s|($base::|($crate::|" "$src/src/main.rs" >"$dst/src/main.rs"
{
    echo "//! Loadout variant made by tools/loadout.sh: \`$base\` with slot $slot forced to \`Arm::$arm\`."
    sed -e "s|loadout: None|loadout: Some(squad::Loadout { slot: $slot, arm: squad::Arm::$arm })|" "$src/src/lib.rs"
} >"$dst/src/lib.rs"
grep -q "$crate::" "$dst/src/main.rs" || { echo "could not rename the crate in $dst/src/main.rs" >&2; exit 1; }
echo "$dst"
