//! Planetiler, run as a process of its own (docs/MODULE_PLAN.md, «Generador»): the Java runtime
//! unpacked from its download, the official `planetiler.jar` and the Protomaps profile compiled into
//! this module, in a folder laid out as the profile reads it (`data/sources/`).

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::read::GzDecoder;
use geko_module_sdk::{GekoError, Result};

use crate::config::PlanetilerConfig;

/// The profile, with the patches of `planetiler/patches/` (`build.rs`); empty if the module was
/// compiled without it.
pub const PROFILE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/protomaps-basemap.jar"));

/// The name the profile gives the area, and so the merged extract (`data/sources/<area>.osm.pbf`).
pub const AREA: &str = "scope";
pub const OUTPUT: &str = "scope.pmtiles";
pub const LOG: &str = "planetiler.log";

/// A fingerprint of the profile, for the key of the stage that runs it (FNV-1a).
pub fn profile_fingerprint() -> String {
    let hash = PROFILE.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x} {}", PROFILE.len())
}

pub fn has_profile() -> Result<()> {
    if PROFILE.is_empty() {
        return Err(GekoError::Config(
            "this tiles module was compiled without its Planetiler profile: build it with ci/profile.sh and compile with GEKO_TILES_PROFILE=<its jar> (ci/modules.sh does)".into(),
        ));
    }
    Ok(())
}

/// Unpacks the Java runtime of `tarball` into `dir`.
pub fn unpack_jre(tarball: &Path, dir: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(File::open(tarball)?)));
    archive.set_preserve_permissions(true);
    archive.unpack(dir).map_err(|e| GekoError::Data(format!("{}: {e}", tarball.display())))?;
    java(dir).map(|_| ())
}

/// The `java` of a runtime unpacked into `dir` (`<dir>/jdk-…-jre/bin/java`).
pub fn java(dir: &Path) -> Result<PathBuf> {
    for entry in fs::read_dir(dir)? {
        let java = entry?.path().join("bin").join("java");
        if java.is_file() {
            return Ok(java);
        }
    }
    Err(GekoError::Data(format!("no bin/java in the Java runtime unpacked in {}", dir.display())))
}

/// What a run reads, by the name the profile gives it under `data/sources/`.
pub struct Sources {
    pub osm: PathBuf,
    /// Every file of the layers of Natural Earth.
    pub natural_earth: Vec<PathBuf>,
    /// The osmdata polygons, cut: every file of both shapefiles.
    pub osmdata: Vec<PathBuf>,
    pub landcover: PathBuf,
    pub pgf_encoding: PathBuf,
    pub qrank: Option<PathBuf>,
}

pub struct Run<'a> {
    pub java: &'a Path,
    pub planetiler: &'a Path,
    pub profile: &'a Path,
    pub config: &'a PlanetilerConfig,
    pub format: &'a str,
}

/// Runs Planetiler in `dir`, which it leaves with `OUTPUT` and its log in `LOG`.
pub fn run(run: &Run, sources: &Sources, dir: &Path) -> Result<()> {
    let data = dir.join("data").join("sources");
    fs::create_dir_all(data.join("ne"))?;
    link(&sources.osm, &data.join(format!("{AREA}.osm.pbf")))?;
    for file in &sources.natural_earth {
        link(file, &data.join("ne").join(file.file_name().expect("a file")))?;
    }
    for file in &sources.osmdata {
        link(file, &data.join(file.file_name().expect("a file")))?;
    }
    link(&sources.landcover, &data.join("daylight-landcover.gpkg"))?;
    link(&sources.pgf_encoding, &data.join("pgf-encoding.zip"))?;
    if let Some(qrank) = &sources.qrank {
        link(qrank, &data.join("qrank.csv.gz"))?;
    }

    let classpath = std::env::join_paths([run.planetiler, run.profile]).map_err(|e| GekoError::Other(e.into()))?;
    let mut command = Command::new(run.java);
    command
        .current_dir(dir)
        .arg(format!("-Xmx{}", run.config.memory))
        // The profile reads fonts with java.awt: without a display, nor the X11 libraries that the
        // image of geko lacks.
        .arg("-Djava.awt.headless=true")
        .arg("-cp")
        .arg(classpath)
        .arg("com.protomaps.basemap.Basemap")
        .arg(format!("--area={AREA}"))
        .arg(format!("--maxzoom={}", crate::geo::MAX_ZOOM))
        // The whole world: its shapefile reader drops what does not touch the bounds
        // (docs/MODULE_PLAN.md, «Entradas globales»); the scope is set by the cut and the conversion.
        .arg("--bounds=world")
        .arg(format!("--output={OUTPUT}"))
        .arg(format!("--storage={}", run.config.storage.as_str()))
        .arg(format!("--tile_format={}", run.format))
        .arg("--force")
        .stdin(Stdio::null());
    if run.config.threads > 0 {
        command.arg(format!("--threads={}", run.config.threads));
    }
    let log = File::create(dir.join(LOG))?;
    command.stdout(log.try_clone()?).stderr(log);
    tracing::info!(command = ?command, "running Planetiler");
    let status = command.status().map_err(|e| GekoError::Data(format!("cannot start {}: {e}", run.java.display())))?;
    // Its temporary files, as large as the extract: not part of what it leaves.
    let _ = fs::remove_dir_all(dir.join("data").join("tmp"));
    if !status.success() || !dir.join(OUTPUT).is_file() {
        let end = tail(&dir.join(LOG), 30);
        // The Java runtime reads the fonts of the profile with the fontconfig of the system.
        let hint = if end.contains("Problem reading font data") { "\nInstall libfontconfig1: the Java runtime needs it to read the fonts of the profile." } else { "" };
        return Err(GekoError::Data(format!("Planetiler failed ({status}); the end of {}:\n{end}{hint}", dir.join(LOG).display())));
    }
    Ok(())
}

/// The last lines of a log, without the colours of the terminal.
fn tail(path: &Path, lines: usize) -> String {
    let Ok(file) = File::open(path) else { return String::new() };
    let all: Vec<String> = BufReader::new(file).lines().map_while(std::result::Result::ok).collect();
    let strip = |l: &str| {
        let mut out = String::new();
        let mut chars = l.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                chars.by_ref().find(|c| c.is_ascii_alphabetic());
            } else {
                out.push(c);
            }
        }
        out
    };
    all[all.len().saturating_sub(lines)..].iter().map(|l| strip(l)).collect::<Vec<_>>().join("\n")
}

/// `link` names `target`, which must exist: a symbolic link on Unix, a copy elsewhere.
fn link(target: &Path, link: &Path) -> Result<()> {
    if !target.is_file() {
        return Err(GekoError::Data(format!("{} is missing", target.display())));
    }
    let _ = fs::remove_file(link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    #[cfg(not(unix))]
    fs::copy(target, link)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_log_without_colours() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("log");
        fs::write(&log, "one\ntwo\n\u{1b}[31mthree\u{1b}[0m failed\n").unwrap();
        assert_eq!(tail(&log, 2), "two\nthree failed");
    }

    #[test]
    fn a_runtime_without_java_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(java(tmp.path()).is_err());
        fs::create_dir_all(tmp.path().join("jdk-21-jre/bin")).unwrap();
        fs::write(tmp.path().join("jdk-21-jre/bin/java"), "").unwrap();
        assert_eq!(java(tmp.path()).unwrap(), tmp.path().join("jdk-21-jre/bin/java"));
    }
}
