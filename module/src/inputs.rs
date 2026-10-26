//! What a build downloads besides the OSM extracts (docs/MODULE_PLAN.md, «Entradas globales»,
//! «Generador» and «Estilos»): the global inputs of the profile, Planetiler with its Java runtime and
//! the fonts and sprites of the styles, from their origin. What `planetiler/profile.toml` pins is checked by its sha256; the rest are refreshed as
//! the host's `[data]` says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use geko_module_sdk::bundle::LicenseNotice;
use geko_module_sdk::download::{Checksum, DownloadRequest};
use geko_module_sdk::{GekoError, Result};
use serde::Deserialize;

/// `planetiler/profile.toml`, compiled into the module.
#[derive(Debug, Deserialize)]
pub struct Pins {
    pub profile: ProfilePin,
    pub planetiler: Pinned,
    pub jre: Jre,
    pub natural_earth: NaturalEarth,
    pub assets: AssetsPin,
}

#[derive(Debug, Deserialize)]
pub struct AssetsPin {
    pub commit: String,
    pub base: String,
}

#[derive(Debug, Deserialize)]
pub struct ProfilePin {
    pub commit: String,
}

#[derive(Debug, Deserialize)]
pub struct Pinned {
    #[serde(default)]
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Jre {
    pub release: String,
    /// By platform, as geko names it (`linux-x86_64`).
    #[serde(flatten)]
    pub platforms: BTreeMap<String, Pinned>,
}

#[derive(Debug, Deserialize)]
pub struct NaturalEarth {
    pub base: String,
    /// The files of the layers, by their path under `base`.
    pub sha256: BTreeMap<String, String>,
}

pub fn pins() -> Pins {
    toml::from_str(include_str!("../../planetiler/profile.toml")).expect("planetiler/profile.toml is valid")
}

pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Who a download is for: a source of the tiles, whose license goes into the manifest, or the
/// tools that generate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    NaturalEarth,
    Osmdata,
    Daylight,
    PgfEncoding,
    Qrank,
    Fonts,
    Sprites,
    Planetiler,
    Temurin,
}

impl Source {
    /// Its folder in `raw/`.
    pub fn id(self) -> &'static str {
        match self {
            Source::NaturalEarth => "natural-earth",
            Source::Osmdata => "osmdata",
            Source::Daylight => "daylight",
            Source::PgfEncoding => "pgf-encoding",
            Source::Qrank => "qrank",
            Source::Fonts => "fonts",
            Source::Sprites => "sprites",
            Source::Planetiler => "planetiler",
            Source::Temurin => "temurin",
        }
    }

    /// The license of the data it puts into the tiles or the styles; `None` for the tools.
    pub fn license(self) -> Option<LicenseNotice> {
        let notice = |name: &str, attribution: &str, url: &str| {
            Some(LicenseNotice { source: self.id().into(), name: name.into(), attribution: attribution.into(), url: Some(url.into()) })
        };
        match self {
            Source::NaturalEarth => notice("Public domain", "Made with Natural Earth", "https://www.naturalearthdata.com/about/terms-of-use/"),
            Source::Osmdata => notice("ODbL 1.0", "© OpenStreetMap contributors", "https://osmdata.openstreetmap.de/info/license.html"),
            Source::Daylight => notice("ODbL 1.0", "© OpenStreetMap contributors", "https://daylightmap.org/attribution.html"),
            Source::Qrank => notice("CC0 1.0", "QRank, from Wikimedia page views", "https://github.com/brawer/wikidata-qrank"),
            Source::Fonts => notice("OFL 1.1", "Noto Sans, © The Noto Project Authors", "https://github.com/protomaps/basemaps-assets/blob/main/fonts/OFL.txt"),
            Source::Sprites => notice("MIT", "Icons of Tangram, © Mapzen", "https://github.com/tangrams/icons/blob/master/LICENSE.md"),
            Source::PgfEncoding | Source::Planetiler | Source::Temurin => None,
        }
    }
}

/// One file to download.
#[derive(Debug, Clone)]
pub struct Input {
    pub source: Source,
    /// Relative to the folder of its source in `raw/`.
    pub file: String,
    pub url: String,
    pub sha256: Option<String>,
    /// Without it the build goes on, and says so.
    pub optional: bool,
}

impl Input {
    pub fn path(&self, data_dir: &Path) -> PathBuf {
        raw_dir(data_dir, self.source.id()).join(&self.file)
    }

    pub fn request(&self) -> DownloadRequest {
        DownloadRequest { url: self.url.clone(), checksum: self.sha256.clone().map(Checksum::Sha256) }
    }
}

pub fn raw_dir(data_dir: &Path, source: &str) -> PathBuf {
    data_dir.join("raw").join(source)
}

/// A font or sprite of the styles (`planetiler/assets.tsv`), by its path in the bundle: `fonts/<font
/// stack>/<range>.pbf`, `fonts/OFL.txt` or `sprites/<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub path: String,
    pub kind: AssetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetKind {
    /// Downloaded and checked by its sha256.
    File { source: Source, file: String, url: String, sha256: String },
    /// The same bytes as the asset at this path: a symbolic link of the repository.
    Link(String),
}

/// The fonts and sprites, from `planetiler/assets.tsv` (`ci/assets.sh`).
pub fn assets(pins: &Pins) -> Vec<Asset> {
    let in_bundle = |path: &str| path.replacen("sprites/v4/", "sprites/", 1);
    include_str!("../../planetiler/assets.tsv")
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(path, pin)| {
            let kind = match pin.strip_prefix('@') {
                Some(target) => AssetKind::Link(in_bundle(target)),
                None => {
                    let (source, file) = match path.strip_prefix("fonts/") {
                        Some(file) => (Source::Fonts, file),
                        None => (Source::Sprites, path.rsplit('/').next().unwrap_or(path)),
                    };
                    let url = format!("{}/{}", pins.assets.base.trim_end_matches('/'), path.replace(' ', "%20"));
                    AssetKind::File { source, file: file.to_owned(), url, sha256: pin.to_owned() }
                }
            };
            Asset { path: in_bundle(path), kind }
        })
        .collect()
}

pub const WATER: &str = "water-polygons-split-3857";
pub const LAND: &str = "land-polygons-split-3857";

/// Every input of a build but the OSM extracts. `qrank`: as `[planetiler] qrank` says.
pub fn inputs(pins: &Pins, qrank: bool) -> Result<Vec<Input>> {
    let input = |source, file: &str, url: &str, sha256: Option<&str>| Input {
        source,
        file: file.to_owned(),
        url: url.to_owned(),
        sha256: sha256.map(str::to_owned),
        optional: false,
    };
    let mut list = Vec::new();
    for (path, sha256) in &pins.natural_earth.sha256 {
        let file = path.rsplit('/').next().unwrap_or(path);
        let url = format!("{}/{path}", pins.natural_earth.base.trim_end_matches('/'));
        list.push(input(Source::NaturalEarth, file, &url, Some(sha256)));
    }
    for name in [WATER, LAND] {
        list.push(input(Source::Osmdata, &format!("{name}.zip"), &format!("https://osmdata.openstreetmap.de/download/{name}.zip"), None));
    }
    list.push(input(Source::Daylight, "daylight-landcover.gpkg", "https://r2-public.protomaps.com/datasets/daylight-landcover.gpkg", None));
    list.push(input(Source::PgfEncoding, "pgf-encoding.zip", "https://wipfli.github.io/pgf-encoding/pgf-encoding.zip", None));
    if qrank {
        list.push(Input { optional: true, ..input(Source::Qrank, "qrank.csv.gz", "https://qrank.toolforge.org/download/qrank.csv.gz", None) });
    }
    let planetiler = &pins.planetiler;
    list.push(input(Source::Planetiler, &format!("planetiler-{}.jar", planetiler.version), &planetiler.url, Some(&planetiler.sha256)));
    let platform = platform();
    let jre = pins.jre.platforms.get(&platform).ok_or_else(|| {
        GekoError::Config(format!("no Java runtime for {platform} in planetiler/profile.toml: the tiles cannot be built on this platform"))
    })?;
    let file = jre.url.rsplit('/').next().unwrap_or(&jre.url);
    list.push(input(Source::Temurin, file, &jre.url, Some(&jre.sha256)));
    for asset in assets(pins) {
        if let AssetKind::File { source, file, url, sha256 } = asset.kind {
            list.push(input(source, &file, &url, Some(&sha256)));
        }
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pins_are_complete() {
        let pins = pins();
        assert_eq!(pins.profile.commit.len(), 40);
        assert!(pins.jre.platforms.contains_key("linux-x86_64") && pins.jre.platforms.contains_key("linux-aarch64"));
        // The 11 layers of the profile, each with its geometry, index, attributes, projection and encoding.
        assert_eq!(pins.natural_earth.sha256.len(), 11 * 5);
        for (path, sha) in pins.natural_earth.sha256.iter().chain(pins.jre.platforms.values().map(|p| (&p.url, &p.sha256))) {
            assert!(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()), "{path}");
        }
    }

    #[test]
    fn inputs_by_source() {
        if !pins().jre.platforms.contains_key(&platform()) {
            assert!(inputs(&pins(), true).is_err());
            return;
        }
        let list = inputs(&pins(), false).unwrap();
        assert!(!list.iter().any(|i| i.source == Source::Qrank));
        let qrank = inputs(&pins(), true).unwrap();
        assert!(qrank.iter().any(|i| i.source == Source::Qrank && i.optional));
        let ne = list.iter().find(|i| i.file == "ne_10m_land.shp").unwrap();
        assert_eq!(ne.url, "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/10m_physical/ne_10m_land.shp");
        assert_eq!(ne.path(Path::new("/d")), Path::new("/d/raw/natural-earth/ne_10m_land.shp"));
        let jar = list.iter().find(|i| i.source == Source::Planetiler).unwrap();
        assert_eq!((jar.file.as_str(), jar.sha256.is_some()), ("planetiler-0.10.2.jar", true));
        let font = list.iter().find(|i| i.source == Source::Fonts && i.file == "Noto Sans Regular/0-255.pbf").unwrap();
        assert_eq!(font.url, format!("{}/fonts/Noto%20Sans%20Regular/0-255.pbf", pins().assets.base));
        assert_eq!(font.path(Path::new("/d")), Path::new("/d/raw/fonts/Noto Sans Regular/0-255.pbf"));
        assert!(list.iter().any(|i| i.source == Source::Sprites && i.file == "light@2x.png"));
    }

    #[test]
    fn the_assets_of_the_styles() {
        let assets = assets(&pins());
        let files = assets.iter().filter(|a| matches!(a.kind, AssetKind::File { .. })).count();
        // Four font stacks of 256 ranges, their license, and 5 sprites at two densities, in JSON and PNG.
        assert_eq!((assets.len(), files), (4 * 256 + 1 + 20, 4 * 256 + 1 + 20 - 252));
        let link = assets.iter().find(|a| a.path == "fonts/Noto Sans Devanagari Regular v1/0-255.pbf").unwrap();
        assert_eq!(link.kind, AssetKind::Link("fonts/Noto Sans Regular/0-255.pbf".into()));
        for a in &assets {
            if let AssetKind::Link(target) = &a.kind {
                assert!(assets.iter().any(|b| &b.path == target && matches!(b.kind, AssetKind::File { .. })), "{}", a.path);
            }
        }
        assert!(assets.iter().any(|a| a.path == "sprites/dark@2x.json"));
    }
}
