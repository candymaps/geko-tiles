#!/usr/bin/env bash
# The fonts and sprites of the Protomaps styles, pinned: protomaps/basemaps-assets at the commit of
# [assets] in planetiler/profile.toml. Each installation downloads them from there, piece by piece,
# and checks each one against planetiler/assets.tsv, which this script writes: one line per file,
# `<path> TAB <sha256>`, or `<path> TAB @<path>` for a symbolic link of the repository (the Devanagari
# font links most of its ranges to Noto Sans Regular), which is not downloaded but served as its
# target:
#
#   ci/assets.sh
#
# Run it after changing the commit. The clone stays in $CARGO_TARGET_DIR/assets.
set -euo pipefail
cd "$(dirname "$0")/.."

target=${CARGO_TARGET_DIR:-target}
field() { tr -d '\r' < "$2" | sed -n "/^\[$1\]/,/^\[/s/^$3 *= *\"\(.*\)\"/\1/p" | head -1; }
repository=$(field assets planetiler/profile.toml repository)
commit=$(field assets planetiler/profile.toml commit)

src=$target/assets/basemaps-assets
mkdir -p "$(dirname "$src")"
if [ ! -d "$src/.git" ]; then
    git -c core.autocrlf=false clone -q --filter=blob:none --no-checkout "$repository" "$src"
fi
git -C "$src" cat-file -e "$commit^{commit}" 2> /dev/null || git -C "$src" fetch -q origin "$commit"
git -C "$src" -c core.autocrlf=false checkout -q --force "$commit"

# The fonts the styles name, their license, and the sprites of the version the styles use.
out=$PWD/planetiler/assets.tsv
cd "$src"
git ls-tree -r HEAD fonts sprites/v4 | LC_ALL=C sort -t$'\t' -k2 | while IFS=$'\t' read -r meta path; do
    case $path in fonts/*/*.pbf | fonts/OFL.txt | sprites/v4/*) ;; *) continue ;; esac
    if [ "${meta%% *}" = 120000 ]; then
        printf '%s\t@%s\n' "$path" "$(realpath -m --relative-to=. "$(dirname "$path")/$(readlink "$path")")"
    else
        printf '%s\t%s\n' "$path" "$(sha256sum < "$path" | cut -d' ' -f1)"
    fi
done > "$out"
echo "pinned $(grep -vc $'\t@' "$out") assets and $(grep -c $'\t@' "$out") links of ${commit:0:7} in planetiler/assets.tsv"
