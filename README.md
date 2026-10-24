<p align="center">
  <img height="100" src="https://placehold.co/170x30/transparent/A4CC47?text=Geko+Tiles&font=raleway" alt="geko-tiles">
</p>
<h3 align="center">The vector tiles module of geko: a map of your scope, served by geko alone.</h3>
<p align="center">It builds vector tiles from OpenStreetMap and Natural Earth for the countries of geko.toml, and serves them without decoding a single byte.</p>
<p align="center">
  <a href="https://github.com/candymaps/geko-tiles/releases/latest"><img src="https://img.shields.io/github/v/release/candymaps/geko-tiles?label=version&color=blue&style=plastic" alt="Version"></a>
  <a href="https://www.gnu.org/licenses/agpl-3.0"><img src="https://img.shields.io/badge/license-AGPL--3.0-orange?style=plastic" alt="License"></a>
  <a href="https://github.com/orgs/candymaps/packages?repo_name=geko-tiles"><img src="https://img.shields.io/badge/module-ghcr.io-informational?style=plastic&logo=docker" alt="Module"></a>
  <a href="https://github.com/candymaps/geko-tiles/actions/workflows/publish.yml"><img src="https://github.com/candymaps/geko-tiles/actions/workflows/publish.yml/badge.svg?branch=main" alt="publish"></a>
</p>

# geko-tiles

The vector tiles module of [geko](https://github.com/candymaps/geko). **Status: a skeleton.** It meets the contract of the host and passes its conformance suite, declares its routes and answers 503 to all of them: it builds nothing yet, and no version has been published. The plan, with its decisions and measurements, is in [docs/MODULE_PLAN.md](docs/MODULE_PLAN.md) (in Spanish).

What it will serve, under `/tiles/`:

- **Vector tiles** (`/tiles/{z}/{x}/{y}`), MVT by default and MLT as an option, up to zoom 14, with the [Protomaps Basemap](https://github.com/protomaps/basemaps) schema.
- **A TileJSON** (`/tiles/tilejson.json`) with the attribution and versioned tile URLs, cached forever by clients and CDNs.
- **Styles, fonts and sprites**, as static data: any style can paint the tiles, and the server never knows which one.

On top of the same foundation as the rest of geko:

- **Fully automatic:** it downloads what the scope of `geko.toml` needs (the OSM extracts, the Natural Earth layers it uses, the water and land polygons, cut to the scope on the machine), builds the tiles and keeps them up to date.
- **Fast and lightweight:** a tile is one lookup in an index pinned in RAM and a copy of its bytes, already compressed.
- **Any combination of countries and regions**, in a single build.

[Planetiler](https://github.com/onthegomap/planetiler) generates the tiles, during the build only, in a process of its own: the JVM never enters geko. The module downloads the official `planetiler.jar` and a Java runtime (Temurin) from their origin, checked against fixed checksums.

## Installation and usage

Not available yet: no version of the module has been published, and the host keeps it disabled. Once it is, it will be enabled like any module of geko, in `geko.toml`:

```toml
[modules]
tiles = true
```

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
| [Protomaps Basemap](https://github.com/protomaps/basemaps) | The Planetiler profile: what goes into each layer of the tiles |
