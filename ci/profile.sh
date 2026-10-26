#!/usr/bin/env bash
# The Planetiler profile of the module: Protomaps Basemap at the commit of planetiler/profile.toml,
# with planetiler/patches/ applied, compiled into a jar without dependencies (Planetiler itself is
# downloaded by each installation), and the layers of its styles, for every flavor and language:
#
#   ci/profile.sh <folder>   leaves <folder>/protomaps-basemap.jar and <folder>/styles/
#
# It compiles in Docker (maven:3-eclipse-temurin-21, the Java of Planetiler 0.10.2), with the jar
# dated as the commit of the profile, so that the same sources give the same jar; the styles, with
# planetiler/styles.ts in Docker too (node:22-slim, the packages of its package-lock.json). The
# clone and the Maven and npm caches stay in $CARGO_TARGET_DIR/profile, for the next run.
set -euo pipefail
cd "$(dirname "$0")/.."

out=${1:?usage: ci/profile.sh <folder>}
target=${CARGO_TARGET_DIR:-target}
work=$target/profile
field() { tr -d '\r' < "$2" | sed -n "/^\[$1\]/,/^\[/s/^$3 *= *\"\(.*\)\"/\1/p" | head -1; }
repository=$(field profile planetiler/profile.toml repository)
commit=$(field profile planetiler/profile.toml commit)

mkdir -p "$work" "$out"
work=$(cd "$work" && pwd)
src=$work/basemaps
clone() {
    rm -rf "$src"
    git -c core.autocrlf=false clone -q --filter=blob:none --no-checkout "$repository" "$src"
}
[ -d "$src/.git" ] || clone
# A clone restored by the cache of the CI (Swatinem/rust-cache, which keeps target/) may come back
# broken, and then the fetch fails with «not our ref»: it is cloned again.
if ! git -C "$src" cat-file -e "$commit^{commit}" 2> /dev/null \
    && ! git -C "$src" fetch -q origin "$commit"; then
    clone
    git -C "$src" cat-file -e "$commit^{commit}" 2> /dev/null || git -C "$src" fetch -q origin "$commit"
fi
git -C "$src" -c core.autocrlf=false checkout -q --force "$commit"
git -C "$src" clean -qfdx
git -C "$src" apply --whitespace=nowarn "$PWD"/planetiler/patches/*.patch

date=$(git -C "$src" log -1 --format=%cI "$commit")
mkdir -p "$work/m2"
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e MAVEN_CONFIG=/tmp/.m2 \
    -v "$src/tiles:/src" -v "$work/m2:/tmp/.m2" -w /src maven:3-eclipse-temurin-21 \
    mvn -q -B -Dmaven.repo.local=/tmp/.m2/repository -DskipTests -Dspotless.check.skip=true \
    -Dspotless.apply.skip=true -Djacoco.skip=true "-Dproject.build.outputTimestamp=$date" package
cp "$src/tiles/target/protomaps-basemap-HEAD.jar" "$out/protomaps-basemap.jar"
echo "built the profile (${commit:0:7}, $(ls planetiler/patches/*.patch | wc -l) patches) in $out/protomaps-basemap.jar"

cp planetiler/styles.ts "$src/styles/geko-styles.ts"
rm -rf "$out/styles" && mkdir -p "$out/styles" "$work/npm"
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e npm_config_cache=/tmp/.npm \
    -v "$src/styles:/src" -v "$work/npm:/tmp/.npm" -v "$(cd "$out/styles" && pwd):/out" -w /src node:22-slim \
    sh -c 'npm ci --no-audit --no-fund --silent && npx tsx geko-styles.ts /out'
echo "generated $(find "$out/styles" -name '*.json' | wc -l) styles in $out/styles"
