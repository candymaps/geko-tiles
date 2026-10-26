//! `geko-tiles.toml`: the configuration of the tiles module, beside `geko.toml`. The host keeps the
//! scope, the data directory and how data are downloaded; this file, the format of the tiles, where
//! the OSM extracts come from and what Planetiler may use while it builds.

use geko_module_sdk::{GekoError, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `mvt`, which every client reads, or `mlt`, about 10 % smaller, which only MapLibre GL JS
    /// 5.12 and later reads (docs/MODULE_PLAN.md, «Formato de las teselas»).
    pub format: Format,
    /// Where clients fetch the tiles, as `tilejson.json` writes it: `/tiles`, on the host that
    /// answered, or an absolute URL (a CDN in front of geko, a prefix of a reverse proxy).
    pub url: String,
    pub osm: OsmConfig,
    pub planetiler: PlanetilerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self { format: Format::default(), url: "/tiles".into(), osm: OsmConfig::default(), planetiler: PlanetilerConfig::default() }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Mvt,
    Mlt,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Mvt => "mvt",
            Format::Mlt => "mlt",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OsmConfig {
    /// `auto`, Geofabrik, or the URL of a mirror with its layout: the same extracts as the
    /// geocoder's, which share their downloads.
    pub mirror: String,
}

impl Default for OsmConfig {
    fn default() -> Self {
        Self { mirror: "auto".into() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlanetilerConfig {
    /// Heap of the Java process (`-Xmx`): `4g`, `512m`…
    pub memory: String,
    /// Threads; 0: one per core.
    pub threads: usize,
    /// Where Planetiler keeps its temporary data: `mmap` (files mapped, the least memory) or `ram`.
    pub storage: Storage,
    /// Download qrank (the popularity of places and POIs by Wikidata), which orders them better.
    /// The build goes on without it if it cannot be downloaded.
    pub qrank: bool,
}

impl Default for PlanetilerConfig {
    fn default() -> Self {
        Self { memory: "4g".into(), threads: 0, storage: Storage::Mmap, qrank: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    Mmap,
    Ram,
}

impl Storage {
    pub fn as_str(self) -> &'static str {
        match self {
            Storage::Mmap => "mmap",
            Storage::Ram => "ram",
        }
    }
}

impl Config {
    /// `file` names the file in errors.
    pub fn parse(text: &str, file: &str) -> Result<Self> {
        let config: Self = toml::from_str(text).map_err(|e| GekoError::Config(if file.is_empty() { e.to_string() } else { format!("{file}: {e}") }))?;
        let memory = &config.planetiler.memory;
        let valid = memory.len() > 1
            && memory[..memory.len() - 1].bytes().all(|b| b.is_ascii_digit())
            && memory.ends_with(['g', 'm', 'G', 'M']);
        if !valid {
            return Err(GekoError::Config(format!("[planetiler] memory = {memory:?}: a size such as \"4g\" or \"512m\"")));
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_options() {
        let c = Config::parse("", "").unwrap();
        assert_eq!((c.format, c.osm.mirror.as_str(), c.planetiler.memory.as_str(), c.planetiler.storage), (Format::Mvt, "auto", "4g", Storage::Mmap));
        let c = Config::parse("format = \"mlt\"\n[planetiler]\nmemory = \"12g\"\nthreads = 4\nstorage = \"ram\"\nqrank = false\n", "").unwrap();
        assert_eq!((c.format, c.planetiler.threads, c.planetiler.storage, c.planetiler.qrank), (Format::Mlt, 4, Storage::Ram, false));
        assert!(Config::parse("[planetiler]\nmemory = \"lots\"\n", "").is_err());
        let err = Config::parse("formats = \"mvt\"", "geko-tiles.toml").unwrap_err().to_string();
        assert!(err.contains("geko-tiles.toml"), "{err}");
    }
}
