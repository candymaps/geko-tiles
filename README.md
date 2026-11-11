<p align="center">
  <img height="100" src="https://placehold.co/170x30/transparent/A4CC47?text=Geko+Tiles&font=raleway" alt="geko-tiles">
</p>
<h3 align="center">The vector tiles module of geko.</h3>
<p align="center">A map of your scope, served by geko alone.</p>
<p align="center">
  <a href="https://github.com/candymaps/geko-tiles/releases/latest"><img src="https://img.shields.io/github/v/release/candymaps/geko-tiles?label=version&color=blue&style=plastic" alt="Version"></a>
  <a href="https://www.gnu.org/licenses/agpl-3.0"><img src="https://img.shields.io/badge/license-AGPL--3.0-orange?style=plastic" alt="License"></a>
  <a href="https://github.com/orgs/candymaps/packages?repo_name=geko-tiles"><img src="https://img.shields.io/badge/module-ghcr.io-informational?style=plastic&logo=docker" alt="Module"></a>
  <a href="https://github.com/candymaps/geko-tiles/actions/workflows/publish.yml"><img src="https://github.com/candymaps/geko-tiles/actions/workflows/publish.yml/badge.svg?branch=main" alt="publish"></a>
</p>

# geko-tiles

The vector tiles module of [geko](https://github.com/candymaps/geko). **Status: published, and enabled by default in geko.** It meets the contract of the host and passes its conformance suite; it downloads what the scope needs, generates the tiles with Planetiler, leaves its part of the bundle (all of Spain in 14 minutes) and serves it, with styles, fonts and sprites. The machine that builds needs `libfontconfig1` (the Java runtime reads the fonts of the profile with it). The plan, with its decisions and measurements, is in [docs/MODULE_PLAN.md](docs/MODULE_PLAN.md) (in Spanish).

What it serves, under `/tiles/`:

- **Vector tiles** (`/tiles/{z}/{x}/{y}`), MVT by default and MLT as an option, up to zoom 14, with the [Protomaps Basemap](https://github.com/protomaps/basemaps) schema. A tile that does not exist answers 204, which clients take as empty.
- **A TileJSON** (`/tiles/tilejson.json`) with the attribution, the layers, the bounds of the scope and versioned tile URLs (`?v=<bundle>`): those tiles are cached forever by clients and CDNs, and a new bundle changes the URLs.
- **Styles** (`/tiles/styles/{flavor}/{lang}.json`, listed at `/tiles/styles.json`): those of Protomaps, in 5 flavors (`light`, `dark`, `white`, `grayscale`, `black`) and 41 languages, with their **fonts** (`/tiles/fonts/…`) and **sprites** (`/tiles/sprites/…`). They are data: any other MapLibre style can paint the tiles, pointing to `tilejson.json`, and the server never knows which one.

- **Status and statistics** (`/tiles/status.json`, `/tiles/stats.json`): the bundle loaded, its tiles, layers, styles and sources and the options in effect; and, since the module started, the requests, errors and time of every route, the tiles of each zoom level, the last hour minute by minute and the styles asked for. They feed the dashboard of geko.

On top of the same foundation as the rest of geko:

- **Fully automatic:** it downloads what the scope of `geko.toml` needs (the OSM extracts, the Natural Earth layers it uses, the water and land polygons, cut to the scope on the machine), builds the tiles and keeps them up to date.
- **Fast and lightweight:** a tile is one lookup in an index pinned in RAM and a copy of its bytes, already compressed: about 2 µs, and millions of tiles per second (all of Spain: 1.17 GB on disk, 9 MB in RAM).
- **Any combination of countries and regions**, in a single build.

[Planetiler](https://github.com/onthegomap/planetiler) generates the tiles, during the build only, in a process of its own: the JVM never enters geko. The module downloads the official `planetiler.jar` and a Java runtime (Temurin) from their origin, checked against fixed checksums.

## Installation and usage

It is enabled like any module of geko, in `geko.toml` (it is on by default):

```toml
[modules]
tiles = true
```

Its own options go in `geko-tiles.toml`, beside `geko.toml`; every one is optional:

```toml
format = "mvt"        # or "mlt": about 10 % smaller, read only by MapLibre GL JS 5.12 and later
url = "/tiles"        # where clients fetch the tiles, fonts and sprites: relative, under the host each request reached
                      # (Host, or X-Forwarded-Host and X-Forwarded-Proto behind a proxy), so a map loads a style by its
                      # URL as it is; absolute (https://maps.example.com/tiles) behind a CDN or a proxy with a prefix

[osm]
mirror = "auto"       # Geofabrik, or the URL of a mirror with its layout (the extracts are shared with the geocoder)

[planetiler]
memory = "4g"         # heap of the Java process (-Xmx)
threads = 0           # 0: one per core
storage = "mmap"      # or "ram": where Planetiler keeps its temporary data
qrank = true          # download QRank to order places and POIs; the build goes on without it

[styles]
editable = false      # accept saving and deleting your own styles over HTTP: turn it on only with [serve] api_keys in geko.toml
```

Downloading requires `[data] accept_licenses = true` in `geko.toml`, as for every module. Besides OSM and the inputs of Planetiler, it downloads the fonts (Noto Sans, OFL) and sprites (icons of Tangram, MIT) of the styles, from [protomaps/basemaps-assets](https://github.com/protomaps/basemaps-assets) at a fixed commit. Planetiler's memory, threads and storage change how fast the build runs and how much memory it takes, never the tiles.

If [osmdata](https://osmdata.openstreetmap.de) does not answer, the build keeps the water and land polygons it has and, without them, downloads the copy pinned in `planetiler/profile.toml` from an archive, checked by its sha256. That copy is older than what osmdata publishes, and the report of the build says so. The next build that reaches osmdata asks it for changes, as usual.

### Your own styles

Besides the styles of Protomaps, any MapLibre style can paint the tiles. With `[styles] editable = true`, the dashboard (Tiles, Styles) saves them, or `POST /tiles/styles/custom/<name>/save` with the JSON as its body; `POST /tiles/styles/custom/<name>/delete` deletes one. They are served at `/tiles/styles/custom/<name>.json` and listed in `/tiles/styles.json`. A change needs no build, and every replica answers it at once.

The source `protomaps` of a style is always the tiles of the bundle in use. Without `glyphs` or `sprite`, it gets the fonts and the sprites of the bundle; with them, its own, served from wherever they point. geko checks that a client can load the style and that its layers name sources it has, not the values of paint and layout.

### API keys

With `[serve] api_keys` in `geko.toml`, ask for a style or for `tilejson.json` with `?api_key=<key>`: every URL in the answer carries that key, so that the map can load the tiles, fonts and sprites.

### Compression

Tiles, fonts and the styles of Protomaps are sent as stored, with `Content-Encoding: gzip`, whatever the client asks: browsers and map libraries read them as they are, and with `curl` use `--compressed`.

### PMTiles

`geko tiles export <file.pmtiles>` writes the tiles of the current bundle to one PMTiles archive, to serve them with something else.

## License

geko-tiles is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only). You may use, study, modify and share it freely. If you distribute it, modified or not, you must pass on its source code under the same license; if you modify it and let others use it over a network, you must offer them the source code of your version. Running it, or modifying it for internal use only, requires nothing more.

The tiles are a Produced Work of OpenStreetMap (ODbL) and carry its attribution. Planetiler and the Java runtime are not redistributed: each installation downloads them from their origin, with their own licenses. Contributions are accepted under the same license, from the collaborators of the repository; see [CONTRIBUTING.md](CONTRIBUTING.md).

Read the full terms in [LICENSE](LICENSE).

## Technologies

| Technology | What it is used for |
|---|---|
| **Rust** (2024 edition) | The module: its build stages and the service of the tiles |
| `geko-module-sdk` | The contract with the host, at the tag `sdk-v<version>` of [geko](https://github.com/candymaps/geko) |
| [Planetiler](https://github.com/onthegomap/planetiler) (Java) | Generating the tiles, as a subprocess of the build |
| [Protomaps Basemap](https://github.com/protomaps/basemaps) | The Planetiler profile (what goes into each layer of the tiles) and the styles that paint them |
