#!/usr/bin/env bash
# The checks of every change of the tiles module, the same locally and on the CI (GitHub Actions):
# the CI files only prepare Rust and the cache, then run this script.
#
#   ci/test.sh [--variant <variants>]
#
# --variant, what is checked, comma-separated (default: code,contracts,profile):
#   code          clippy (warnings are errors), dependency rules (the module speaks to the host
#                 only through the contract of the SDK), and licenses and security advisories
#                 (cargo-deny)
#   contracts     unit tests and the conformance suite of the module contract
#   profile       the Planetiler profile: planetiler/patches/ apply to the Protomaps commit of
#                 planetiler/profile.toml and it compiles (ci/profile.sh, in Docker)
#   golden        builds the tiles of Baleares on the frozen data named in golden/snapshot and fails
#                 if they are not, byte for byte, those of golden/baseline.json (where they differ:
#                 by zoom, the tiles of a few places, the metadata, the fonts and sprites). It needs
#                 Docker for the profile, the snapshot in $GEKO_GOLDEN_DIR/snapshots or in the
#                 registry (oras login ghcr.io), and downloads what profile.toml pins
#
# The golden, besides the environment of ci/golden.sh:
#   GEKO_GOLDEN_FRESH=1    downloads the sources afresh instead of the snapshot (ci/golden-snapshot.sh);
#                          without either, it uses the downloads there are in GEKO_GOLDEN_DIR
#   GEKO_GOLDEN_UPDATE=1   writes what it measures as the new golden/baseline.json
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=ci/golden.sh
. ci/golden.sh

lint() {
    rustup component list --installed 2>/dev/null | grep -q '^clippy' || rustup component add clippy
    cargo clippy --workspace --all-targets --locked -- -D warnings
}

test() {
    cargo test --workspace --locked
}

direct_dependencies() { cargo tree -p "$1" -e normal --depth 1 --prefix none --locked | tail -n +2 | cut -d' ' -f1 | sort -u; }

# Fails, naming them, if `found` (one per line) has any name not in `allowed` (space-separated).
only() {
    local what=$1 allowed=$2 found=$3 name bad=""
    for name in $found; do
        [[ " $allowed " == *" $name "* ]] || bad="$bad $name"
    done
    [ -z "$bad" ] || { echo "$what:$bad" >&2; return 1; }
}

# What the dependency graph can show of the rules of the host (its AGENTS.md): the module knows the
# host only through the contract of the SDK, and downloads only with its Downloader (geko-fetch, as
# the geocoder): no HTTP client of its own. A new dependency is either a rule broken or a decision to
# write down here.
architecture() {
    local failed=0
    only "geko-module-tiles depends on" "arc-swap flate2 geko-fetch geko-module-sdk memmap2 osmpbf serde serde_json tar toml tracing zip" "$(direct_dependencies geko-module-tiles)" || failed=1
    only "geko-module-tiles-dylib depends on" "geko-module-sdk geko-module-tiles tracing-subscriber" "$(direct_dependencies geko-module-tiles-dylib)" || failed=1
    return $failed
}

CARGO_DENY_VERSION=0.20.2

# Licenses of the dependencies (deny.toml) and security advisories of the versions in Cargo.lock.
deny() {
    local bin
    bin=$(command -v cargo-deny || true)
    if [ -z "$bin" ] || ! "$bin" --version | grep -q " $CARGO_DENY_VERSION\$"; then
        local dir="${CARGO_HOME:-$HOME/.cargo}/bin" name="cargo-deny-$CARGO_DENY_VERSION-x86_64-unknown-linux-musl"
        mkdir -p "$dir"
        curl -fsSL "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/$name.tar.gz" \
            | tar -xzf - -C "$dir" --strip-components=1 "$name/cargo-deny"
        bin="$dir/cargo-deny"
    fi
    "$bin" --locked check
}

# Puts the frozen downloads of golden/snapshot into the raw/ of the golden, unless they are already
# there; the pinned downloads stay, as a cache.
restore_snapshot() {
    local dir=$1 file sha archive source
    file=$(snapshot_field golden/snapshot file) sha=$(snapshot_field golden/snapshot sha256)
    [ "$(cat "$dir/raw/.snapshot" 2>/dev/null)" = "$sha" ] && return 0
    archive="$dir/snapshots/$file"
    if [ ! -f "$archive" ]; then
        mkdir -p "$dir/snapshots"
        if ! (cd "$dir/snapshots" && oras pull --no-tty "$(snapshot_ref "$file")" > /dev/null); then
            echo "golden data $file is neither in $dir/snapshots nor in $(snapshot_ref "$file") (oras login ghcr.io?); GEKO_GOLDEN_FRESH=1 downloads the sources" >&2
            return 1
        fi
    fi
    echo "$sha  $archive" | sha256sum -c --quiet - || { echo "$archive does not match its sha256" >&2; return 1; }
    for source in $GOLDEN_FROZEN qrank; do rm -rf "${dir:?}/raw/$source"; done
    mkdir -p "$dir/raw"
    tar -xzf "$archive" -C "$dir/raw"
    echo "$sha" > "$dir/raw/.snapshot"
}

# The sha256 of the frozen downloads in the raw/ of the golden, to check that the build used them as
# they are: one it does not take as its own (a record that does not match its file) is downloaded
# again, and the golden would no longer run on frozen data.
frozen_sums() {
    local dir=$1 source
    for source in $GOLDEN_FROZEN; do
        (cd "$dir/raw" && find "$source" -type f ! -name '*.raw.json' -print0 | sort -z | xargs -0 sha256sum)
    done
}

golden() {
    local dir source frozen=
    dir=$(golden_dir)
    mkdir -p "$dir"
    if [ "${GEKO_GOLDEN_FRESH:-0}" = 1 ]; then
        for source in $GOLDEN_FROZEN qrank; do rm -rf "${dir:?}/raw/$source"; done
        rm -f "$dir/raw/.snapshot"
    elif [ -f golden/snapshot ]; then
        restore_snapshot "$dir"
        frozen=1
    fi
    [ -z "$frozen" ] || frozen_sums "$dir" > "$dir/frozen.sha256"
    golden_build "$dir" "$frozen" > "$dir/build.log" || { tail -n 30 "$dir/build.log" >&2; return 1; }
    if [ -n "$frozen" ] && ! frozen_sums "$dir" | diff -u "$dir/frozen.sha256" -; then
        echo "the build downloaded again some of the frozen data (above): the golden did not run on golden/snapshot" >&2
        return 1
    fi
    cargo run --release --locked -q -p geko-module-tiles --example golden -- report "$dir" > "$dir/report.json"
    if [ "${GEKO_GOLDEN_UPDATE:-0}" = 1 ]; then
        cp "$dir/report.json" golden/baseline.json
        echo "golden/baseline.json updated"
    elif [ ! -f golden/baseline.json ]; then
        echo "no golden/baseline.json: GEKO_GOLDEN_UPDATE=1 writes it" >&2
        return 1
    elif ! diff -u golden/baseline.json "$dir/report.json"; then
        echo "the tiles of the golden changed (- baseline, + now); if it is wanted, GEKO_GOLDEN_UPDATE=1 updates the baseline" >&2
        return 1
    fi
}

ALL_VARIANTS=code,contracts,profile,golden
VARIANTS=code,contracts,profile

usage() { echo "usage: $0 [--variant $ALL_VARIANTS]" >&2; }

# Whether the variant `$1` was asked for.
wants() { [[ ",$VARIANTS," == *",$1,"* ]]; }

while [ $# -gt 0 ]; do
    case $1 in
        --variant) VARIANTS=${2:?--variant takes a list}; shift 2 ;;
        --variant=*) VARIANTS=${1#*=}; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done
for variant in ${VARIANTS//,/ }; do
    [[ ",$ALL_VARIANTS," == *",$variant,"* ]] || { echo "unknown variant $variant" >&2; usage; exit 2; }
done

# Runs a check in a subshell, where errexit stops it at its first failure as when it ran alone,
# and goes on with the next one: the summary names the checks that failed.
FAILED=()
run() {
    local name=$1 status
    shift
    echo "==> $name"
    set +e
    (
        set -e
        "$@"
    )
    status=$?
    set -e
    [ "$status" = 0 ] || FAILED+=("$name")
}

if wants code; then
    run lint lint
    run architecture architecture
    run deny deny
fi
wants contracts && run test test
wants profile && run profile ci/profile.sh "${CARGO_TARGET_DIR:-target}/profile/out"
wants golden && run golden golden
if [ ${#FAILED[@]} -gt 0 ]; then
    echo "failed: ${FAILED[*]}" >&2
    exit 1
fi
echo "passed"
