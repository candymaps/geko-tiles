#!/usr/bin/env bash
# The checks of ci/test.sh that what changed since a commit can fail, for the CI of pull requests
# (whether to run them):
#
#   ci/affected.sh <base> [<head>]
#
# prints the arguments for ci/test.sh (`--variant code,contracts`) for what changed from <base> to
# <head> (without it, to the working tree), or nothing when no check covers it (only
# documentation and what the checks do not build: the CI and the publication).
set -euo pipefail
cd "$(dirname "$0")/.."

base=${1:?usage: ci/affected.sh <base> [<head>]}
range=("$base" ${2:+"$2"})

while IFS= read -r file; do
    case $file in
        *.md | LICENSE | .github/* | ci/affected.sh | ci/modules.sh | ci/oras.sh) ;;
        *) echo "--variant code,contracts"; exit 0 ;;
    esac
done < <(git diff --name-only "${range[@]}")
