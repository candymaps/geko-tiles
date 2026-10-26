# What ci/test.sh and ci/golden-snapshot.sh share about the golden of the tiles: where its data live,
# what is frozen and where the snapshots are published (sourced by both).
#   GEKO_GOLDEN_DIR        data directory, kept between runs as a cache (default /var/lib/geko/golden/tiles)
#   GEKO_GOLDEN_REGISTRY   where the snapshots are published (default ghcr.io/candymaps/geko-tiles/golden)

# shellcheck source=ci/oras.sh
. ci/oras.sh
# Where the golden snapshots are published: <registry>:<snapshot>, private.
GOLDEN_REGISTRY="${GEKO_GOLDEN_REGISTRY:-ghcr.io/candymaps/geko-tiles/golden}"
# The scope of the golden, as the build example takes it: Baleares, as the golden of the geocoder.
GOLDEN_SCOPE="ES:ES-PM"
# What is frozen: the downloads of raw/ without a sha256 pinned in planetiler/profile.toml, which can
# change from one day to the next. The pinned ones (Natural Earth, Planetiler, the JRE, the fonts and
# sprites) are downloaded and checked by their sha256; qrank is left out (golden/geko-tiles.toml).
GOLDEN_FROZEN="osm osmdata daylight pgf-encoding"

golden_dir() { echo "${GEKO_GOLDEN_DIR:-/var/lib/geko/golden/tiles}"; }

# Value of `key` in a snapshot file of `key=value` lines.
snapshot_field() { sed -n "s/^$2=//p" "$1"; }

# The artifact of the snapshot archive `$1`: golden-tiles-ES-PM-20261025.tar.gz is
# <registry>:tiles-ES-PM-20261025.
snapshot_ref() { local tag=${1#golden-}; echo "$GOLDEN_REGISTRY:${tag%.tar.gz}"; }

# The Planetiler profile that the module is compiled with: GEKO_TILES_PROFILE, or the one of
# ci/profile.sh in the target directory, built if it is not there.
golden_profile() {
    local out="${CARGO_TARGET_DIR:-target}/profile/out"
    if [ -n "${GEKO_TILES_PROFILE:-}" ]; then
        echo "$GEKO_TILES_PROFILE"
        return
    fi
    [ -f "$out/protomaps-basemap.jar" ] || ci/profile.sh "$out" >&2
    # Absolute: build.rs runs in module/, where the default target/ of the CI does not resolve.
    echo "$(cd "$out" && pwd)/protomaps-basemap.jar"
}

# Builds the tiles of the golden in its data directory, with everything redone (the stamps only know
# the version of the code of each stage, not the code): `--frozen` keeps the downloads there are.
golden_build() {
    local dir=$1 frozen=$2 profile
    profile=$(golden_profile)
    GEKO_TILES_PROFILE=$profile GEKO_TILES_CONFIG=$PWD/golden/geko-tiles.toml \
        cargo run --release --locked -q -p geko-module-tiles --example build -- "$dir" $GOLDEN_SCOPE --force ${frozen:+--frozen}
}
