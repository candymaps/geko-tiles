# geko-tiles

The vector tiles module of [geko](https://github.com/candymaps/geko), a modular, multi-country geospatial engine: vector tiles of the scope of `geko.toml`, built with Planetiler and the Protomaps Basemap profile, and served by geko alone. Goal: ultra-efficient in CPU, memory, latency and disk, decided always with measurements, and any style can paint the tiles.

The plan, with what is decided, what is open, the steps and the measurements, is in [docs/MODULE_PLAN.md](docs/MODULE_PLAN.md). Read it before designing or implementing anything, and update it when a step is finished or a decision is made. The contract with the host is in `docs/MODULES.md` of the host.

## Environment

- Building and testing (cargo, `ci/test.sh`) happen in WSL Debian; `geko` runs in Docker. The code lives in each machine's repository folder, which in WSL is `/mnt/<drive>/…`. The host sits beside this repository (`../geko`): `make start-dev` there builds this module from here.
- Use `CARGO_TARGET_DIR=$HOME/.cache/geko-target` (outside `/mnt/c`, to build fast).
- On the `D:` machine there is no C compiler in WSL (`build-essential` is missing): Zig is the temporary compiler and linker, with `CC=~/.local/bin/zigcc AR=~/.local/bin/zigar CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=~/.local/bin/zigcc`. From PowerShell, run each WSL command from a script (`wsl -d Debian -- bash <script>`): the quotes and `$` of an inline command get mangled on the way.
- The host is [candymaps/geko](https://github.com/candymaps/geko): the SDK (`geko-module-sdk`) is a git dependency of it (`Cargo.toml`), at the tag `sdk-<contract>`, fetched with the git of WSL (`.cargo/config.toml`). `ci/modules.sh` installs the signing tool at the commit `Cargo.lock` resolved for that tag.
- The private key that signs the module is in `~/.config/geko/pack-signing.key` (and in the `GEKO_PACK_SIGNING_KEY` GitHub secret); the public one, in `TRUSTED_KEYS` of `geko-artifact` in the host. Never print it or copy it into the repository.
- Nothing of geko loose in `~`: everything generated goes under `/var/lib/geko`, never in the repository. The tests of Planetiler (Docker, `eclipse-temurin:21-jre`) live in `/var/lib/geko/spike-tiles`, and are deleted when done.
- Docker Desktop, needed by the tests of Planetiler, may be stopped: start it before (`Docker Desktop.exe`, under `%LOCALAPPDATA%\Programs\DockerDesktop`).
- Always measure performance in `--release`, and Planetiler builds with the same machine, threads and `-Xmx`.
- Publishing: `publish.yml`, on every push to `main` (the merge of a pull request that passed the checks), publishes the module (`ci/modules.sh`, compiled on the Debian of the image of geko, to `ghcr.io/candymaps/geko-tiles`) if the push touches something that goes into it, with the version in `Cargo.toml` (`<version>-<commit>`). Raising the version in `[workspace.package]` is making a release: in addition, `<version>` on the module and the git tag `v<version>` on the pushed commit, with its GitHub release. The CI signs with the `GEKO_PACK_SIGNING_KEY` secret.
- GitHub CI (`ci.yml`) checks every pull request: `ci/test.sh` if it changes what the checks build (`ci/affected.sh`), nothing if it only changes documentation, the CI or the publication. Its `test` job and `protected.yml` (which fails a pull request by anyone but the owner that changes `LICENSE`, `CONTRIBUTING.md`, `README.md`, `AGENTS.md`, `CLAUDE.md`, `.github/`, `ci/`, or what sets how strict the checks are: `deny.toml`, `clippy.toml`, `.cargo/`, `rust-toolchain.toml`) are the statuses that the protection of `main` and `dev` requires; `pr-comment.yml` says on the pull request whether the checks passed, failed or were not needed.
- Checks, the same locally and in CI (GitHub only calls the script): `ci/test.sh [--variant code,contracts]`. `code`: clippy, dependency rules (the module speaks to the host only through the SDK) and cargo-deny; `contracts`: tests and the conformance suite of the module contract.

## Branches and commits (Gitflow)

- **Nothing is ever pushed directly to `main`**: no commits, no pushes.
- All new development starts from `dev`, in its own branch with a Gitflow name: `feature/<name>`, `bugfix/<name>`… It ends with a PR from that branch to `dev`.
- A release (a push to `main`) is always a PR from `dev` to `main`.
- All commits follow [Conventional Commits](https://www.conventionalcommits.org/): `<type>(<optional scope>): <description>`, with type `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci` or `chore`. A breaking change takes `!` after the type (`feat!: …`) or a `BREAKING CHANGE:` footer. The title of each PR follows the same format.

## Key rules

- geko is a host and its modules: this repository is the `tiles` module, enabled by `[modules]` of `geko.toml`. The host knows it only through the contract of `geko-module-sdk` (`Module`), and the module uses nothing of the host but the SDK: `ci/test.sh` (`architecture`) checks it. It serves its routes under `/tiles/`, reads its own `geko-tiles.toml` and builds its own part of the bundle. Every option of the module is documented in its README, not in `geko.toml` or the README of the host. A disabled module does not download, does not instantiate, does not open files and does not register routes.
- The service is pure Rust: Planetiler (Java) runs only in the build, as a subprocess, and the JVM never enters the process or the image of geko. The module downloads the official `planetiler.jar` and the Temurin JRE from their origin, with their sha256 fixed in the code; only the Protomaps profile, with our patches, goes into the package of the module. No package of our own for Planetiler or for data.
- Every download goes through the `Downloader` of the host that the SDK passes: no HTTP client of the module, and never `--download` of Planetiler. So `offline`, the `raw/` cache, `accept_licenses` and the licenses of the manifest hold. Downloading requires `accept_licenses = true`. No download protected by a CAPTCHA or other access control is automated.
- Global inputs are cut to the scope on the machine, never published already cut: only the Natural Earth layers the profile uses, and the osmdata polygons cut by a stage of the build; after cutting, only the record of the download is kept (`Downloader::keeps_removed`). qrank is optional: if it does not answer, the build goes on without it and says so.
- One Planetiler run per scope: the OSM extracts of the scope are merged first into one PBF, by a stage of the module. Planetiler runs with `--bounds=world` (its shapefile reader drops what does not touch the bounds: with those of the scope, Sardinia vanished from the low zooms of Baleares); the scope is set by the cut of the global inputs and by the conversion, which keeps from z6 up only the tiles that touch it. Tiles are never merged after Planetiler.
- Build incremental by input hash (PROJECT.md §8 of the host): each stage leaves a `<dir>.stamp.json` stamp and is skipped if neither its code nor its inputs change (the key of the Planetiler stage carries the versions of Planetiler and of the profile, and their options); `geko build --force` redoes everything. The build must be deterministic: the same inputs give the same bytes.
- Serving a tile is a lookup in the index and a copy of its bytes: no decoding, no compression, no allocation beyond the body. Hot part (pinned in RAM) and cold part (mapped) in separate files.
- MVT by default, so that any client can read the tiles; MLT only as an option of `geko-tiles.toml`. Styles are data, never code of the server.
- Build and serving on different machines. Bundles are immutable and versioned; a bundle in use is never modified. Production serving on Linux.
- Selective abstraction: only the ports of the SDK are traits. Everything else is plain code; its trait is extracted when a second real implementation appears.
- Licenses (`docs/MODULE_PLAN.md`, «Licencias»): the code is AGPL-3.0-only (`LICENSE`) and contributions are accepted under that same license, with no CLA: only the collaborators of the repository may open pull requests. The tiles are a Produced Work of OSM (ODbL): their attribution goes in `tilejson.json`, in the bundle manifest and in the style. Every source declares its license in the manifest.

## Optimizations: what worked and what did not

Before optimizing, check this list so as not to repeat what was already discarded. When you try something, add it here with its measurement, whether it works or not. The full measurements are in `docs/MODULE_PLAN.md`, «Medidas».

**Worked:**
- Zoom 14 as the maximum (2026-10-24, Baleares): half the size of zoom 15 (25.0 MB against 52.8 MB), with overzoom in the client.
- One Planetiler run with the extracts merged first (2026-10-24, La Rioja and Navarra): 2 min 2 s against 3 min 37 s for one run per extract, which pays the global inputs each time.
- The official `planetiler.jar` with the profile jar without dependencies (2026-10-24, La Rioja): the same tiles, byte for byte, as the profile jar with dependencies. No package of our own is needed.
- Natural Earth by layers (2026-10-24, Baleares): the 11 shapefiles the profile uses, of version 5.1.2 like the gpkg, instead of the whole gpkg (446 MB): Natural Earth from 71 to 6 s, the whole build from 2 min to 1 min. With `--bounds=world` and the osmdata polygons cut to the scope first (the 1.9 GB become 4.3 MB, read in 1 s instead of 20 s), every tile from z8 to z14 is byte for byte the original one, in 58 s.

**Did not work, part two:**
- Natural Earth by layers with the `--bounds` of the scope (2026-10-24, Baleares): the shapefile reader of Planetiler only reads what touches the bounds, and Sardinia vanished from z5 (4.4 % of the land of the tile). The gpkg reader reads everything when the table has no spatial index.

**Did not work (do not repeat):**
- Recompressing the tiles (2026-10-24, Baleares, z14): brotli-11 only saves 4.9 %, zopfli 1.6 %, gzip-9 nothing. The gzip of Planetiler stays.
- One Planetiler run per extract and a merge of the tiles (2026-10-24, La Rioja and Navarra): 6 % of the features of the shared tiles appear in both runs (the extracts carry whole the ways that cross their border), and removing them by id does not give the same tiles as one run (lines are joined differently when the zoom goes down). `--clip` does not avoid it.
- `osmium merge` without `--bounds`: the merged PBF has no bounds in its header, and Planetiler takes them from the nodes and builds far too much (47,365 tiles against 14,076).
