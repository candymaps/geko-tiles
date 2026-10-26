#!/usr/bin/env bash
# The data of the golden of the tiles: the downloads of Baleares without a pinned sha256, frozen in
# an archive that golden/snapshot names with its sha256, so that only a change in the code can change
# what ci/test.sh --variant golden compares. Not a check: it prepares what the check runs on.
#
#   ci/golden-snapshot.sh renew [--only-local]   # all of it
#   ci/golden-snapshot.sh create                 # freeze the downloads of the golden as its data
#   ci/golden-snapshot.sh publish                # push them to the registry (private)
#
# `renew` downloads the sources afresh and measures the new baseline on them
# (GEKO_GOLDEN_FRESH=1 GEKO_GOLDEN_UPDATE=1 ci/test.sh --variant golden), freezes those same
# downloads (`create`), checks that the frozen data give that same baseline and publishes them
# (`publish`), unless `--only-local`: then the snapshot stays on this machine, and the golden of
# another machine cannot find it until it is published. Then commit golden/snapshot with
# golden/baseline.json.
#
# The osmdata polygons are frozen cut to the scope (a few MB instead of 1.8 GB): cut again, they give
# the same files. Publishing needs `oras login ghcr.io` with a token with write:packages.
# Environment: that of ci/golden.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=ci/golden.sh
. ci/golden.sh

create() {
    local dir name sha stage source
    dir=$(golden_dir)
    [ -f "$dir/tiles/osmdata.stamp.json" ] || { echo "no build in $dir: run GEKO_GOLDEN_FRESH=1 ci/test.sh --variant golden first" >&2; return 1; }
    stage="$dir/freeze"
    rm -rf "$stage" && mkdir -p "$stage"
    for source in $GOLDEN_FROZEN; do
        [ -d "$dir/raw/$source" ] || { echo "no downloads of $source in $dir/raw" >&2; return 1; }
        if [ "$source" = osmdata ]; then
            cargo run --release --locked -q -p geko-module-tiles --example golden -- osmdata "$dir" "$stage/osmdata"
        else
            cp -r "$dir/raw/$source" "$stage/$source"
        fi
    done
    name="golden-tiles-${GOLDEN_SCOPE#*:}-$(date -u +%Y%m%d).tar.gz"
    mkdir -p "$dir/snapshots"
    tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$stage" -cf - . | gzip -n > "$dir/snapshots/$name"
    rm -rf "$stage"
    sha=$(sha256sum "$dir/snapshots/$name" | cut -d' ' -f1)
    cat > golden/snapshot <<EOF
# Frozen downloads of ${GOLDEN_SCOPE#*:} that golden/baseline.json is measured on (ci/test.sh, ci/golden-snapshot.sh).
file=$name
sha256=$sha
EOF
    echo "$dir/snapshots/$name ($(du -h "$dir/snapshots/$name" | cut -f1)): publish it with ci/golden-snapshot.sh publish"
}

# Pushes the snapshot that golden/snapshot names, signed by nothing but its sha256 in the repository:
# the golden checks it before using it. Its package is linked to the repository, so that the CI
# reads it with its own token; it is created private, and stays so.
publish() {
    local dir file
    dir=$(golden_dir) file=$(snapshot_field golden/snapshot file)
    [ -f "$dir/snapshots/$file" ] || { echo "$dir/snapshots/$file does not exist: run ci/golden-snapshot.sh create first" >&2; return 1; }
    (cd "$dir/snapshots" && oras push --no-tty "$(snapshot_ref "$file")" "$file" \
        --artifact-type application/vnd.geko.golden-snapshot.v1 \
        --annotation "org.opencontainers.image.source=https://github.com/candymaps/geko-tiles" \
        --annotation "org.opencontainers.image.description=Frozen downloads of the golden of the tiles (OpenStreetMap and osmdata ODbL 1.0, © OpenStreetMap contributors; Daylight landcover ODbL 1.0)" \
        --annotation "org.opencontainers.image.created=1970-01-01T00:00:00Z")
    echo "published $(snapshot_ref "$file")"
}

renew() {
    local only_local=$1
    GEKO_GOLDEN_FRESH=1 GEKO_GOLDEN_UPDATE=1 ci/test.sh --variant golden
    create
    # The frozen data must give the baseline just measured on the whole downloads.
    ci/test.sh --variant golden
    if [ "$only_local" = 1 ]; then
        echo "not published (--only-local): the golden of another machine cannot find it until ci/golden-snapshot.sh publish"
    else
        publish
    fi
    echo "commit golden/snapshot and golden/baseline.json"
}

usage() { echo "usage: $0 renew|create|publish [--only-local]" >&2; }

command=${1:-}
shift || true
only_local=0
while [ $# -gt 0 ]; do
    case $1 in
        --only-local) only_local=1; shift ;;
        *) usage; exit 2 ;;
    esac
done
[ "$only_local" = 0 ] || [ "$command" = renew ] || { echo "--only-local is for renew" >&2; usage; exit 2; }
case $command in
    renew) renew "$only_local" ;;
    create | publish) "$command" ;;
    *) usage; exit 2 ;;
esac
