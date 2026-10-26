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

The vector tiles module of [geko](https://github.com/candymaps/geko). **Status: it builds and serves the tiles, with styles, fonts and sprites.** It meets the contract of the host and passes its conformance suite; it downloads what the scope needs, generates the tiles with Planetiler, leaves its part of the bundle (all of Spain in 14 minutes) and serves it, but no version has been published. The machine that builds needs `libfontconfig1` (the Java runtime reads the fonts of the profile with it). The plan, with its decisions and measurements, is in [docs/MODULE_PLAN.md](docs/MODULE_PLAN.md) (in Spanish).

What it serves, under `/tiles/`:

- **Vector tiles** (`/tiles/{z}/{x}/{y}`), MVT by default and MLT as an option, up to zoom 14, with the [Protomaps Basemap](https://github.com/protomaps/basemaps) schema. A tile that does not exist answers 204, which clients take as empty.
- **A TileJSON** (`/tiles/tilejson.json`) with the attribution, the layers, the bounds of the scope and versioned tile URLs (`?v=<bundle>`): those tiles are cached forever by clients and CDNs, and a new bundle changes the URLs.
- **Styles** (`/tiles/styles/{flavor}/{lang}.json`, listed at `/tiles/styles.json`): those of Protomaps, in 5 flavors (`light`, `dark`, `white`, `grayscale`, `black`) and 41 languages, with their **fonts** (`/tiles/fonts/…`) and **sprites** (`/tiles/sprites/…`). They are data: any other MapLibre style can paint the tiles, pointing to `tilejson.json`, and the server never knows which one.

On top of the same foundation as the rest of geko:

- **Fully automatic:** it downloads what the scope of `geko.toml` needs (the OSM extracts, the Natural Earth layers it uses, the water and land polygons, cut to the scope on the machine), builds the tiles and keeps them up to date.
- **Fast and lightweight:** a tile is one lookup in an index pinned in RAM and a copy of its bytes, already compressed: about 2 µs, and millions of tiles per second (all of Spain: 1.17 GB on disk, 9 MB in RAM).
- **Any combination of countries and regions**, in a single build.

[Planetiler](https://github.com/onthegomap/planetiler) generates the tiles, during the build only, in a process of its own: the JVM never enters geko. The module downloads the official `planetiler.jar` and a Java runtime (Temurin) from their origin, checked against fixed checksums.

## Installation and usage

Not available yet: no version of the module has been published, and the host keeps it disabled. Once it is, it will be enabled like any module of geko, in `geko.toml`:

```toml
[modules]
tiles = true
```

Its own options go in `geko-tiles.toml`, beside `geko.toml`; every one is optional:

```toml
format = "mvt"        # or "mlt": about 10 % smaller, read only by MapLibre GL JS 5.12 and later
url = "/tiles"        # where clients fetch the tiles, fonts and sprites, as tilejson.json and the styles write it: absolute
                      # (https://maps.example.com/tiles) for clients that load a style or a TileJSON by its URL (MapLibre
                      # needs the tiles and the sprite absolute), behind a CDN or behind a proxy with a prefix

[osm]
mirror = "auto"       # Geofabrik, or the URL of a mirror with its layout (the extracts are shared with the geocoder)

[planetiler]
memory = "4g"         # heap of the Java process (-Xmx)
threads = 0           # 0: one per core
storage = "mmap"      # or "ram": where Planetiler keeps its temporary data
qrank = true          # download QRank to order places and POIs; the build goes on without it
```

Downloading requires `[data] accept_licenses = true` in `geko.toml`, as for every module. Besides OSM and the inputs of Planetiler, it downloads the fonts (Noto Sans, OFL) and sprites (icons of Tangram, MIT) of the styles, from [protomaps/basemaps-assets](https://github.com/protomaps/basemaps-assets) at a fixed commit. Planetiler's memory, threads and storage change how fast the build runs and how much memory it takes, never the tiles.

## License

geko-tiles is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only). You may use, study, modify and share it freely. If you distribute it, modified or not, you must pass on its source code under the same license; if you modify it and let others use it over a network, you must offer them the source code of your version. Running it, or modifying it for internal use only, requires nothing more.

The tiles are a Produced Work of OpenStreetMap (ODbL) and carry its attribution. Planetiler and the Java runtime are not redistributed: each installation downloads them from their origin, with their own licenses. Contributions are accepted under the same license, from the collaborators of the repository; see [CONTRIBUTING.md](CONTRIBUTING.md).

Read the full terms in [LICENSE](LICENSE).

## Technologies

| Technology | What it is used for |
|---|---|
| **Rust** (2024 edition) | The module: its build stages and the service of the tiles |
| `geko-module-sdk` | The contract with the host, at the tag `sdk-<contract>` of [geko](https://github.com/candymaps/geko) |
| [Planetiler](https://github.com/onthegomap/planetiler) (Java) | Generating the tiles, as a subprocess of the build |
| [Protomaps Basemap](https://github.com/protomaps/basemaps) | The Planetiler profile (what goes into each layer of the tiles) and the styles that paint them |
