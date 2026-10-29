//! What the tiles module does for `geko plan`, `fetch`, `build` and `purge` (docs/MODULE_PLAN.md,
//! step 4): download the extracts of the scope and the global inputs, cut the osmdata polygons to
//! the scope, merge the extracts, run Planetiler once and convert what it leaves into the part of
//! the bundle, with the fonts and sprites of the styles beside it. Each stage leaves a stamp beside its folder and is skipped when neither its code nor
//! its inputs changed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use geko_fetch::{HttpDownloader, HttpDownloaderConfig, RefreshPolicy};
use geko_module_sdk::bundle::LicenseNotice;
use geko_module_sdk::download::{Checksum, DownloadRequest, Downloader};
use geko_module_sdk::incremental::{self, Key};
use geko_module_sdk::module::{BundlePart, Built, DataPolicy, Refresh};
use geko_module_sdk::progress::{self, Unit};
use geko_module_sdk::scope::Scope;
use geko_module_sdk::stages::StageLog;
use geko_module_sdk::{GekoError, Result};

use crate::config::Config;
use crate::extracts::{self, Catalog};
use crate::geo::BBox;
use crate::inputs::{self, AssetKind, Input, Pins, Source};
use crate::{assets, pbf, planetiler, shapefile, tileset};

/// The versions of the code of each stage: raising one repeats it and the stages after it.
const SCOPE_CODE: &str = "1";
const CUT_CODE: &str = "1";
const MERGE_CODE: &str = "1";
const RUNTIME_CODE: &str = "1";
const PLANETILER_CODE: &str = "1";
const CONVERT_CODE: &str = "1";
const ASSETS_CODE: &str = "1";

const STEPS: u32 = 6;

/// Where the fonts and sprites go in the bundle.
pub const ASSETS_PART: &str = "tiles/assets";

/// What every step reads: the configuration of the module, and what the host tells it.
pub struct Setup {
    pub config: Config,
    pub scope: Scope,
    pub data_dir: PathBuf,
    pub data: DataPolicy,
}

impl Setup {
    /// The folder of the work files of the module.
    pub fn work(&self) -> PathBuf {
        self.data_dir.join("tiles")
    }

    fn base(&self) -> String {
        match self.config.osm.mirror.as_str() {
            "auto" => extracts::DEFAULT_BASE.to_owned(),
            mirror => mirror.trim_end_matches('/').to_owned(),
        }
    }

    /// The OSM extracts go where the geocoder keeps them, so both share them.
    fn osm_dir(&self) -> PathBuf {
        inputs::raw_dir(&self.data_dir, "osm")
    }
}

/// The downloader of a run. With `refresh = "if-changed"`, it asks the sources for changes only when
/// the last check is older than `[data] max_age_days`, or when `refresh_now` (`geko build
/// --refresh`); otherwise it keeps every download and fetches only what is missing.
pub fn downloader(setup: &Setup, refresh_now: bool) -> HttpDownloader {
    HttpDownloader::new(HttpDownloaderConfig {
        policy: match setup.data.refresh {
            Refresh::Never => RefreshPolicy::Never,
            Refresh::IfChanged if refresh_now || check_due(setup) => RefreshPolicy::IfChanged,
            Refresh::IfChanged => RefreshPolicy::Never,
            Refresh::Always => RefreshPolicy::Always,
        },
        offline: setup.data.offline,
        ..Default::default()
    })
}

/// The downloader that brings back what a stage to repeat reads, if it was removed after
/// publishing: it contacts no source for anything else.
fn restorer(setup: &Setup) -> HttpDownloader {
    HttpDownloader::new(HttpDownloaderConfig { policy: RefreshPolicy::Never, offline: setup.data.offline, restore_removed: true, ..Default::default() })
}

/// When the sources of the module were last checked for changes, all at once.
fn checked_path(setup: &Setup) -> PathBuf {
    setup.work().join(".checked")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn since_check(setup: &Setup) -> Option<u64> {
    let checked = fs::read_to_string(checked_path(setup)).ok()?.trim().parse::<u64>().ok()?;
    Some(now_unix().saturating_sub(checked))
}

pub fn check_due(setup: &Setup) -> bool {
    since_check(setup).is_none_or(|s| s >= setup.data.max_age_days.saturating_mul(86_400))
}

/// The shortest time between two builds of `geko up`.
const MIN_WAIT: std::time::Duration = std::time::Duration::from_secs(3600);

/// How long until `geko up` builds again: when the sources are due for a check. Offline, or with
/// `refresh = "never"`, never.
pub fn next_build(setup: &Setup) -> Option<std::time::Duration> {
    if setup.data.offline || setup.data.refresh == Refresh::Never || setup.scope.is_empty() {
        return None;
    }
    let max = setup.data.max_age_days.saturating_mul(86_400);
    let until = std::time::Duration::from_secs(since_check(setup).map_or(0, |s| max.saturating_sub(s)));
    Some(until.max(MIN_WAIT))
}

fn mark_checked(setup: &Setup) -> Result<()> {
    fs::create_dir_all(setup.work())?;
    fs::write(checked_path(setup), now_unix().to_string())?;
    Ok(())
}

/// Downloading needs the licenses of the sources accepted (`[data] accept_licenses`).
pub fn licenses_accepted(setup: &Setup) -> Result<()> {
    if setup.scope.is_empty() || setup.data.accept_licenses {
        return Ok(());
    }
    Err(GekoError::Config(format!(
        "downloading requires accepting the licenses of the sources of the tiles: {}. Set [data] accept_licenses = true",
        licenses(true).iter().map(|l| format!("{} ({})", l.source, l.name)).collect::<Vec<_>>().join(", ")
    )))
}

const LAST_BUILD: &str = "last_build.tiles.json";

/// Writes the stages of this build for the dashboard: the report of the host is one file that every
/// module's build overwrites, so each module keeps its own.
fn write_last_build(data_dir: &Path, stages: &[geko_module_sdk::stages::StageRecord]) {
    let finished = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let report = serde_json::json!({ "finished_unix": finished, "stages": stages });
    if let Err(e) = std::fs::write(data_dir.join(LAST_BUILD), report.to_string()) {
        tracing::warn!(error = %e, "cannot write the stages of this build");
    }
}

/// The stages of the last build of this module, as `write_last_build` left them; `null` before one.
pub fn last_build(data_dir: &Path) -> serde_json::Value {
    std::fs::read(data_dir.join(LAST_BUILD)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn osm_license() -> LicenseNotice {
    LicenseNotice {
        source: "osm".into(),
        name: "ODbL 1.0".into(),
        attribution: "© OpenStreetMap contributors".into(),
        url: Some("https://www.openstreetmap.org/copyright".into()),
    }
}

/// The design of the styles: CC0, with the attribution Protomaps asks for.
fn styles_license() -> LicenseNotice {
    LicenseNotice {
        source: "protomaps".into(),
        name: "CC0 1.0".into(),
        attribution: "Map styles of Protomaps".into(),
        url: Some("https://github.com/protomaps/basemaps".into()),
    }
}

/// The licenses of the data in the tiles and the styles: OSM, each global source, the design of the
/// styles and their fonts and sprites.
fn licenses(qrank: bool) -> Vec<LicenseNotice> {
    let mut list = vec![osm_license(), styles_license()];
    for source in [Source::NaturalEarth, Source::Osmdata, Source::Daylight, Source::Qrank, Source::Fonts, Source::Sprites] {
        if source != Source::Qrank || qrank {
            list.extend(source.license());
        }
    }
    list
}

/// The sources whose licenses tiles declares: the ones of the bundle manifest that are its own, since
/// the manifest also carries those of the other modules.
pub fn license_sources() -> Vec<String> {
    licenses(true).into_iter().map(|l| l.source).collect()
}

/// The Geofabrik index, downloaded as the geocoder downloads it.
fn catalog(setup: &Setup, dl: &dyn Downloader) -> Result<Catalog> {
    let path = setup.osm_dir().join(extracts::INDEX_FILE);
    dl.fetch(&DownloadRequest { url: format!("{}/{}", setup.base(), extracts::INDEX_FILE), checksum: None }, &path)?;
    Catalog::parse(&fs::read(&path)?)
}

/// A download of the build: where it goes and how to ask for it.
struct Download {
    path: PathBuf,
    request: DownloadRequest,
    optional: bool,
    source: Option<Source>,
}

/// What a fetch leaves: each download by its path, with the identity its source gave it.
#[derive(Default)]
struct Fetched {
    report: String,
    identities: BTreeMap<PathBuf, String>,
    requests: BTreeMap<PathBuf, DownloadRequest>,
    extracts: Vec<PathBuf>,
    missing_optional: Vec<Source>,
}

fn global_downloads(setup: &Setup, pins: &Pins) -> Result<Vec<Download>> {
    Ok(inputs::inputs(pins, setup.config.planetiler.qrank)?
        .into_iter()
        .map(|i: Input| Download { path: i.path(&setup.data_dir), request: i.request(), optional: i.optional, source: Some(i.source) })
        .collect())
}

pub fn plan(setup: &Setup, dl: &dyn Downloader) -> String {
    let mut out = String::new();
    writeln!(out, "Tiles: {} from Planetiler {} and the Protomaps profile, up to zoom {}", setup.config.format.as_str().to_uppercase(), inputs::pins().planetiler.version, crate::geo::MAX_ZOOM).unwrap();
    if setup.scope.is_empty() {
        writeln!(out, "  nothing to build: the scope is empty").unwrap();
        return out;
    }
    writeln!(out, "OSM extracts (shared with the geocoder):").unwrap();
    match catalog(setup, dl).and_then(|c| Ok(c.for_scope(&setup.scope)?.into_iter().map(|e| (e.file_name(), e.url(&setup.base()))).collect::<Vec<_>>())) {
        Ok(list) => list.iter().for_each(|(file, url)| writeln!(out, "  {file:<40} {url}").unwrap()),
        Err(e) => writeln!(out, "  cannot plan: {e}").unwrap(),
    }
    writeln!(out, "Global inputs:").unwrap();
    match global_downloads(setup, &inputs::pins()) {
        Ok(list) => {
            let mut by_source: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
            for d in &list {
                let source = d.source.map_or("osm", Source::id);
                by_source.entry(source).or_insert((0, &d.request.url)).0 += 1;
            }
            for (source, (files, url)) in by_source {
                let what = if files == 1 { url.to_owned() } else { format!("{files} files") };
                writeln!(out, "  {source:<14} {what}").unwrap();
            }
        }
        Err(e) => writeln!(out, "  cannot plan: {e}").unwrap(),
    }
    writeln!(out, "Licenses:").unwrap();
    for l in licenses(setup.config.planetiler.qrank) {
        writeln!(out, "  {:<14} {}: {}", l.source, l.name, l.attribution).unwrap();
    }
    if !setup.data.accept_licenses {
        writeln!(out, "Downloads are blocked until [data] accept_licenses = true (review the licenses above).").unwrap();
    }
    if planetiler::has_profile().is_err() {
        writeln!(out, "This module was compiled without its Planetiler profile: it cannot build.").unwrap();
    }
    out
}

pub fn fetch(setup: &Setup, dl: &dyn Downloader) -> Result<String> {
    fetched(setup, dl).map(|f| f.report)
}

fn fetched(setup: &Setup, dl: &dyn Downloader) -> Result<Fetched> {
    licenses_accepted(setup)?;
    let mut f = Fetched::default();
    if setup.scope.is_empty() {
        return Ok(f);
    }
    let catalog = catalog(setup, dl)?;
    let mut downloads = Vec::new();
    for e in catalog.for_scope(&setup.scope)? {
        let file = e.file_name();
        let url = e.url(&setup.base());
        let md5_path = setup.osm_dir().join(format!("{file}.md5"));
        dl.fetch(&DownloadRequest { url: format!("{url}.md5"), checksum: None }, &md5_path)?;
        let md5 = fs::read_to_string(&md5_path)?
            .split_whitespace()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| GekoError::Data(format!("empty checksum file {}", md5_path.display())))?;
        let path = setup.osm_dir().join(&file);
        f.extracts.push(path.clone());
        downloads.push(Download { path, request: DownloadRequest { url, checksum: Some(Checksum::Md5(md5)) }, optional: false, source: None });
    }
    downloads.extend(global_downloads(setup, &inputs::pins())?);
    progress::expect(downloads.len() as u64);
    let outcomes = download_all(&downloads, dl, setup.data.parallel_downloads);
    let (mut downloaded, mut kept) = (0, 0);
    for (d, outcome) in downloads.into_iter().zip(outcomes) {
        match outcome {
            Ok((sha256, fresh)) => {
                f.identities.insert(d.path.clone(), format!("sha256 {sha256}"));
                if fresh { downloaded += 1 } else { kept += 1 }
                f.requests.insert(d.path, d.request);
            }
            Err(e) if d.optional => {
                tracing::warn!(url = %d.request.url, "{e}; the tiles are built without it");
                writeln!(f.report, "could not download {} ({e}): the tiles are built without it", d.request.url).unwrap();
                f.missing_optional.extend(d.source);
            }
            Err(e) => {
                let hint = if d.source.is_none() { " (if Geofabrik published a new extract meanwhile, run again)" } else { "" };
                return Err(GekoError::Data(format!("{e}{hint}")));
            }
        }
    }
    if dl.checks_for_updates() {
        mark_checked(setup)?;
    }
    writeln!(f.report, "downloads of the tiles: {downloaded} downloaded, {kept} up to date").unwrap();
    Ok(f)
}

/// The sha256 of a download, and whether it was downloaded now.
type Downloaded = (String, bool);

/// Every download, `parallel` at a time (`[data] parallel_downloads`): the sha256 of each one and
/// whether it was downloaded now, in the order of `downloads`. One after another, the thousand
/// fonts and sprites took 3 minutes.
fn download_all(downloads: &[Download], dl: &dyn Downloader, parallel: usize) -> Vec<Result<Downloaded>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<Option<Result<Downloaded>>>> = std::sync::Mutex::new(downloads.iter().map(|_| None).collect());
    std::thread::scope(|threads| {
        for _ in 0..parallel.clamp(1, downloads.len().max(1)) {
            threads.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(d) = downloads.get(i) else { break };
                    let result = match pinned_and_recorded(dl, d) {
                        Some(sha256) => Ok((sha256, false)),
                        None => dl.fetch(&d.request, &d.path).map(|o| (o.sha256, o.downloaded)),
                    };
                    progress::advance(1);
                    results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(result);
                }
            });
        }
    });
    results.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().map(|r| r.expect("every download was tried")).collect()
}

/// The checksum of a download pinned by its sha256 that its record says is there (or was removed
/// after publishing, if the downloader keeps those): its content cannot change, so the source is
/// not asked again. Without it, every check would ask for each of the thousand fonts and sprites.
fn pinned_and_recorded(dl: &dyn Downloader, d: &Download) -> Option<String> {
    let Some(Checksum::Sha256(pin)) = &d.request.checksum else { return None };
    if !d.path.is_file() && !dl.keeps_removed() {
        return None;
    }
    let record: geko_fetch::RawRecord = serde_json::from_slice(&fs::read(geko_fetch::sidecar(&d.path)).ok()?).ok()?;
    let size_ok = fs::metadata(&d.path).map_or(true, |m| m.len() == record.size);
    (record.url == d.request.url && record.sha256.eq_ignore_ascii_case(pin) && size_ok).then_some(record.sha256)
}

/// Brings back the downloads of `paths` removed after publishing, for a stage that reads them.
fn restore(setup: &Setup, fetched: &Fetched, paths: &[&Path]) -> Result<()> {
    let restorer = restorer(setup);
    for path in paths {
        if path.exists() {
            continue;
        }
        let request = fetched.requests.get(*path).ok_or_else(|| GekoError::Data(format!("{} is missing", path.display())))?;
        restorer.fetch(request, path)?;
    }
    Ok(())
}

/// Builds `dir` with `build` into a fresh folder, unless the stage keyed by `key` already did.
/// Returns what `build` reported and whether it was reused. `exclude`: entries of `dir` left out of
/// its digest.
fn stage(dir: &Path, key: String, force: bool, exclude: &[&str], build: impl FnOnce(&Path) -> Result<serde_json::Value>) -> Result<(serde_json::Value, bool)> {
    if !force && let Some(stamp) = incremental::up_to_date(dir, &key) {
        return Ok((stamp.info, true));
    }
    incremental::invalidate(dir)?;
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    let info = build(dir)?;
    Ok((incremental::seal(dir, key, exclude, info)?.info, false))
}

fn up_to_date(reused: bool) -> &'static str {
    if reused { " (up to date)" } else { "" }
}

fn ms(log: &StageLog) -> u128 {
    log.elapsed().as_millis()
}

/// Every file of the downloads of `source`.
fn of(fetched: &Fetched, source: &str, data_dir: &Path) -> Vec<PathBuf> {
    let dir = inputs::raw_dir(data_dir, source);
    fetched.identities.keys().filter(|p| p.starts_with(&dir)).cloned().collect()
}

/// Download, cut, merge, run Planetiler and convert, and leave the part of the bundle.
pub fn build(setup: &Setup, dl: &dyn Downloader, force: bool) -> Result<Built> {
    let mut log = StageLog::new();
    let mut out = String::new();
    if setup.scope.is_empty() {
        return Ok(Built { report: "tiles: nothing to build, the scope is empty\n".into(), ..Default::default() });
    }
    planetiler::has_profile()?;
    let pins = inputs::pins();
    let work = setup.work();
    let data_dir = &setup.data_dir;

    progress::step("downloading the sources of the tiles", 1, STEPS, Unit::Items);
    let f = fetched(setup, dl)?;
    out.push_str(&f.report);
    log.done("fetch", false);

    // The boxes of the extracts, from their headers.
    progress::step("cutting the global inputs to the scope", 2, STEPS, Unit::Items);
    let key = Key::new("scope").add("code", SCOPE_CODE).inputs(&f.extracts, &f.identities)?.finish();
    let (info, reused) = stage(&work.join("scope"), key, force, &[], |_| {
        restore(setup, &f, &f.extracts.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
        let mut boxes = Vec::new();
        for path in &f.extracts {
            boxes.push(pbf::header_bbox(path)?.ok_or_else(|| GekoError::Data(format!("{}: no box in its header", path.display())))?);
        }
        // As text: the keys of the next stages take it as it is, and numbers read back from JSON
        // may differ in the last bit from those written.
        Ok(serde_json::json!({ "boxes": serde_json::to_string(&boxes).expect("boxes serialize") }))
    })?;
    let boxes = info["boxes"].as_str().unwrap_or_default().to_owned();
    let scope: Vec<BBox> = serde_json::from_str(&boxes).map_err(|e| GekoError::Data(format!("the stamp of the scope: {e}")))?;
    log.done("scope", reused);

    let zips: Vec<PathBuf> = [inputs::WATER, inputs::LAND].iter().map(|n| inputs::raw_dir(data_dir, Source::Osmdata.id()).join(format!("{n}.zip"))).collect();
    let cut_dir = work.join("osmdata");
    let key = Key::new("cut").add("code", CUT_CODE).add("scope", &boxes).inputs(&zips, &f.identities)?.finish();
    let (info, reused) = stage(&cut_dir, key, force, &[], |dir| {
        restore(setup, &f, &zips.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
        let cut_boxes: Vec<[f64; 4]> = scope.iter().map(|b| b.scope_tiles().mercator()).collect();
        let mut info = serde_json::Map::new();
        for (zip, name) in zips.iter().zip([inputs::WATER, inputs::LAND]) {
            let cut = shapefile::cut(zip, dir, name, &cut_boxes)?;
            info.insert(name.into(), serde_json::json!([cut.kept, cut.total]));
        }
        Ok(info.into())
    })?;
    writeln!(out, "cut the osmdata polygons to the scope: water {}, land {} records in {} ms{}", info[inputs::WATER][0], info[inputs::LAND][0], ms(&log), up_to_date(reused)).unwrap();
    log.done("cut", reused);

    // One extract is read as it is; several, merged first.
    progress::step("merging the OSM extracts", 3, STEPS, Unit::Items);
    let (osm, osm_identity) = if let [extract] = f.extracts.as_slice() {
        (extract.clone(), f.identities.get(extract).cloned().unwrap_or_default())
    } else {
        let dir = work.join("osm");
        let key = Key::new("merge").add("code", MERGE_CODE).add("scope", &boxes).inputs(&f.extracts, &f.identities)?.finish();
        let (info, reused) = stage(&dir, key, force, &[], |dir| {
            restore(setup, &f, &f.extracts.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
            let paths: Vec<&Path> = f.extracts.iter().map(PathBuf::as_path).collect();
            let union = scope.iter().copied().reduce(|a, b| BBox {
                west: a.west.min(b.west),
                south: a.south.min(b.south),
                east: a.east.max(b.east),
                north: a.north.max(b.north),
            });
            let merged = pbf::merge(&paths, &dir.join(format!("{}.osm.pbf", planetiler::AREA)), union)?;
            Ok(serde_json::json!({ "objects": merged.objects, "repeated": merged.repeated }))
        })?;
        writeln!(out, "merged {} OSM extracts: {} objects, {} repeated, in {} ms{}", f.extracts.len(), info["objects"], info["repeated"], ms(&log), up_to_date(reused)).unwrap();
        (dir.join(format!("{}.osm.pbf", planetiler::AREA)), format!("output {}", incremental::output_of(&dir, &[])?))
    };
    log.done("merge", false);

    // The Java runtime, unpacked once per release.
    progress::step("running Planetiler", 4, STEPS, Unit::Share);
    let tarball = of(&f, Source::Temurin.id(), data_dir).pop().ok_or_else(|| GekoError::Data("no Java runtime downloaded".into()))?;
    let runtime = work.join("runtime").join(&pins.jre.release);
    let key = Key::new("runtime").add("code", RUNTIME_CODE).inputs(std::slice::from_ref(&tarball), &f.identities)?.finish();
    let (_, reused) = stage(&runtime, key, force, &[], |dir| {
        restore(setup, &f, &[&tarball])?;
        planetiler::unpack_jre(&tarball, dir)?;
        Ok(serde_json::Value::Null)
    })?;
    log.done("runtime", reused);
    let profile = work.join("runtime").join("protomaps-basemap.jar");
    if fs::read(&profile).ok().as_deref() != Some(planetiler::PROFILE) {
        fs::write(&profile, planetiler::PROFILE)?;
    }

    let planetiler_jar = of(&f, Source::Planetiler.id(), data_dir).pop().ok_or_else(|| GekoError::Data("no planetiler.jar downloaded".into()))?;
    let natural_earth = of(&f, Source::NaturalEarth.id(), data_dir);
    let landcover = of(&f, Source::Daylight.id(), data_dir).pop().ok_or_else(|| GekoError::Data("no landcover downloaded".into()))?;
    let pgf_encoding = of(&f, Source::PgfEncoding.id(), data_dir).pop().ok_or_else(|| GekoError::Data("no pgf-encoding downloaded".into()))?;
    let qrank = of(&f, Source::Qrank.id(), data_dir).pop();
    let osmdata: Vec<PathBuf> = {
        let mut files: Vec<PathBuf> = fs::read_dir(&cut_dir)?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<_>>()?;
        files.sort();
        files
    };
    let read: Vec<PathBuf> = natural_earth.iter().chain([&landcover, &pgf_encoding, &planetiler_jar]).chain(qrank.iter()).cloned().collect();
    let tiles_dir = work.join("planetiler");
    let config = &setup.config.planetiler;
    // Memory, threads and storage change how Planetiler runs, not what it leaves.
    let key = Key::new("planetiler")
        .add("code", PLANETILER_CODE)
        .add("planetiler", &format!("{} {}", pins.planetiler.version, pins.planetiler.sha256))
        .add("profile", &format!("{} {}", pins.profile.commit, planetiler::profile_fingerprint()))
        .add("jre", &pins.jre.release)
        .add("format", setup.config.format.as_str())
        .add("osm", &osm_identity)
        .add("osmdata", &incremental::output_of(&cut_dir, &[])?)
        .inputs(&read, &f.identities)?
        .finish();
    let (_, reused) = stage(&tiles_dir, key, force, &[planetiler::LOG, "data"], |dir| {
        let mut needed: Vec<&Path> = read.iter().map(PathBuf::as_path).collect();
        needed.push(&osm);
        restore(setup, &f, &needed)?;
        let run = planetiler::Run { java: &planetiler::java(&runtime)?, planetiler: &planetiler_jar, profile: &profile, config, format: setup.config.format.as_str() };
        let sources = planetiler::Sources { osm: osm.clone(), natural_earth: natural_earth.clone(), osmdata: osmdata.clone(), landcover: landcover.clone(), pgf_encoding: pgf_encoding.clone(), qrank: qrank.clone() };
        planetiler::run(&run, &sources, dir)?;
        // Only the tiles are its output: the links to its sources are not.
        fs::remove_dir_all(dir.join("data"))?;
        Ok(serde_json::Value::Null)
    })?;
    let size = fs::metadata(tiles_dir.join(planetiler::OUTPUT)).map(|m| m.len()).unwrap_or(0);
    let qrank_note = if qrank.is_none() { ", without qrank" } else { "" };
    writeln!(out, "planetiler: {} MB of tiles in {} s{qrank_note}{}", size / 1_000_000, log.elapsed().as_secs(), up_to_date(reused)).unwrap();
    log.done("planetiler", reused);

    progress::step("converting the tiles", 5, STEPS, Unit::Share);
    let bundle_dir = work.join("bundle");
    let key = Key::new("convert").add("code", CONVERT_CODE).add("scope", &boxes).add("tiles", &incremental::output_of(&tiles_dir, &[planetiler::LOG, "data"])?).finish();
    let (meta, reused) = stage(&bundle_dir, key, force, &[], |dir| {
        let meta = tileset::convert(&tiles_dir.join(planetiler::OUTPUT), dir, &scope)?;
        serde_json::to_value(meta).map_err(|e| GekoError::Other(e.into()))
    })?;
    writeln!(out, "converted: {} tiles, {} different, {} MB in {} ms{}", meta["tiles"], meta["contents"], meta["bytes"].as_u64().unwrap_or(0) / 1_000_000, ms(&log), up_to_date(reused)).unwrap();
    log.done("convert", reused);

    // The fonts and sprites of the styles, packed.
    progress::step("packing the fonts and sprites of the styles", 6, STEPS, Unit::Items);
    let (sources, links): (Vec<assets::Source>, Vec<(String, String)>) = {
        let (mut sources, mut links) = (Vec::new(), Vec::new());
        for asset in inputs::assets(&pins) {
            match asset.kind {
                AssetKind::File { source, file, .. } => sources.push(assets::Source { path: asset.path, file: inputs::raw_dir(data_dir, source.id()).join(file) }),
                AssetKind::Link(target) => links.push((asset.path, target)),
            }
        }
        (sources, links)
    };
    let files: Vec<PathBuf> = sources.iter().map(|s| s.file.clone()).collect();
    let assets_dir = work.join("assets");
    let key = Key::new("assets").add("code", ASSETS_CODE).add("links", &format!("{links:?}")).inputs(&files, &f.identities)?.finish();
    let (info, reused) = stage(&assets_dir, key, force, &[], |dir| {
        restore(setup, &f, &files.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
        serde_json::to_value(assets::pack(&sources, &links, dir)?).map_err(|e| GekoError::Other(e.into()))
    })?;
    writeln!(out, "packed the fonts and sprites: {} assets, {} different, {} MB in {} ms{}", info["assets"], info["contents"], info["bytes"].as_u64().unwrap_or(0) / 1_000_000, ms(&log), up_to_date(reused)).unwrap();
    log.done("assets", reused);

    let stages = log.into_records();
    write_last_build(&setup.data_dir, &stages);
    Ok(Built {
        report: out,
        parts: vec![BundlePart { from: bundle_dir, to: "tiles".into() }, BundlePart { from: assets_dir, to: ASSETS_PART.into() }],
        meta,
        licenses: licenses(qrank.is_some()),
        stages,
        report_extra: serde_json::Map::from_iter([
            ("planetiler".to_owned(), pins.planetiler.version.clone().into()),
            ("profile".to_owned(), pins.profile.commit.clone().into()),
            ("assets".to_owned(), pins.assets.commit.clone().into()),
        ]),
    })
}

/// After the host published a bundle, removes the downloads of data (`[data] keep_downloads =
/// false`), leaving their records: a stage to repeat brings back what it reads. Planetiler and the
/// Java runtime are kept, as tools. Never while offline: nothing could bring them back.
pub fn published(setup: &Setup) -> Result<String> {
    if setup.data.keep_downloads || setup.data.offline {
        return Ok(String::new());
    }
    let mut out = String::new();
    let mut dirs: Vec<PathBuf> = [Source::NaturalEarth, Source::Osmdata, Source::Daylight, Source::PgfEncoding, Source::Qrank, Source::Fonts, Source::Sprites]
        .iter()
        .map(|s| inputs::raw_dir(&setup.data_dir, s.id()))
        .collect();
    dirs.push(setup.osm_dir());
    let (mut files, mut bytes) = (0, 0);
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries {
            let path = entry?.path();
            // The fonts are in a folder per font stack.
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // The records stay, and so do the small files the geocoder and the plan read.
            if name.ends_with(".raw.json") || name.ends_with(".md5") || name.ends_with(".json") || !path.is_file() {
                continue;
            }
            if !geko_fetch::sidecar(&path).exists() {
                continue;
            }
            bytes += fs::metadata(&path)?.len();
            fs::remove_file(&path)?;
            files += 1;
        }
    }
    if files > 0 {
        writeln!(out, "removed the downloads of the tiles: {files} files, {} MB", bytes / 1_000_000).unwrap();
    }
    Ok(out)
}

/// Work files the configuration no longer asks for: Java runtimes of other releases.
pub fn purge(setup: &Setup) -> Result<String> {
    let mut out = String::new();
    let release = inputs::pins().jre.release;
    let Ok(entries) = fs::read_dir(setup.work().join("runtime")) else { return Ok(out) };
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if path.is_dir() && name != release {
            fs::remove_dir_all(&path)?;
            let _ = fs::remove_file(incremental::stamp_path(&path));
            writeln!(out, "purged {}", path.display()).unwrap();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geko_module_sdk::country::{CountryCode, RegionCode};

    fn setup(data: DataPolicy, dir: &Path) -> Setup {
        let mut scope = Scope::default();
        scope.add(CountryCode::new("ES").unwrap(), vec![RegionCode::new("ES-PM").unwrap()]).unwrap();
        Setup { config: Config::default(), scope, data_dir: dir.to_path_buf(), data }
    }

    struct NoNetwork;

    impl Downloader for NoNetwork {
        fn fetch(&self, req: &DownloadRequest, _: &Path) -> Result<geko_module_sdk::download::DownloadOutcome> {
            Err(GekoError::Data(format!("no network: {}", req.url)))
        }
    }

    #[test]
    fn nothing_is_downloaded_without_the_licenses_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(DataPolicy::default(), tmp.path());
        let err = fetch(&s, &NoNetwork).unwrap_err().to_string();
        assert!(err.contains("accept_licenses = true") && err.contains("ODbL"), "{err}");
        assert!(licenses_accepted(&setup(DataPolicy { accept_licenses: true, ..DataPolicy::default() }, tmp.path())).is_ok());
        let plan = plan(&s, &NoNetwork);
        assert!(plan.contains("cannot plan") && plan.contains("blocked until"), "{plan}");
    }

    #[test]
    fn an_empty_scope_builds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Setup { scope: Scope::default(), ..setup(DataPolicy::default(), tmp.path()) };
        let built = build(&s, &NoNetwork, false).unwrap();
        assert!(built.parts.is_empty());
        assert_eq!(next_build(&s), None);
    }

    #[test]
    fn a_stage_is_skipped_until_its_key_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        let runs = std::cell::Cell::new(0);
        let build = |d: &Path| {
            runs.set(runs.get() + 1);
            fs::write(d.join("x"), "x")?;
            Ok(serde_json::json!({ "n": runs.get() }))
        };
        assert_eq!(stage(&dir, "a".into(), false, &[], build).unwrap(), (serde_json::json!({ "n": 1 }), false));
        assert_eq!(stage(&dir, "a".into(), false, &[], build).unwrap(), (serde_json::json!({ "n": 1 }), true));
        assert!(!stage(&dir, "a".into(), true, &[], build).unwrap().1);
        assert!(!stage(&dir, "b".into(), false, &[], build).unwrap().1);
        assert_eq!(runs.get(), 3);
    }
}
