//! The cut of the osmdata polygons (docs/MODULE_PLAN.md, «Entradas globales»): they only exist for
//! the whole planet, split in a grid, so the shapefile inside each zip is copied keeping only the
//! records whose box touches the scope. Records are copied byte for byte, renumbered, with their
//! index and attributes, so the same zip and scope give the same files.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use geko_module_sdk::{GekoError, Result};

const HEADER: usize = 100;

pub struct Cut {
    pub kept: u64,
    pub total: u64,
}

/// Copies the shapefile of `zip` (one `.shp` with its `.shx`, `.dbf`, `.prj` and `.cpg`) into
/// `out_dir/<name>.*`, keeping the records whose box touches one of `boxes` (in the units of the
/// shapefile: `[xmin, ymin, xmax, ymax]`).
pub fn cut(zip: &Path, out_dir: &Path, name: &str, boxes: &[[f64; 4]]) -> Result<Cut> {
    let bad = |what: String| GekoError::Data(format!("{}: {what}", zip.display()));
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(zip)?)).map_err(|e| bad(e.to_string()))?;
    let base = archive
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .find_map(|n| n.strip_suffix(".shp").map(str::to_owned))
        .ok_or_else(|| bad("no .shp inside".into()))?;
    let out = |ext: &str| out_dir.join(format!("{name}.{ext}"));

    let shp = archive.by_name(&format!("{base}.shp")).map_err(|e| bad(e.to_string()))?;
    let (kept, total) = cut_shp(BufReader::with_capacity(1 << 20, shp), &out("shp"), &out("shx"), boxes).map_err(|e| bad(format!("{base}.shp: {e}")))?;
    let dbf = archive.by_name(&format!("{base}.dbf")).map_err(|e| bad(e.to_string()))?;
    cut_dbf(BufReader::with_capacity(1 << 20, dbf), &out("dbf"), &kept, total).map_err(|e| bad(format!("{base}.dbf: {e}")))?;
    for ext in ["prj", "cpg"] {
        let Ok(mut entry) = archive.by_name(&format!("{base}.{ext}")) else { continue };
        io::copy(&mut entry, &mut File::create(out(ext))?)?;
    }
    Ok(Cut { kept: kept.len() as u64, total })
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    match r.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// Copies the records of `shp` that touch `boxes` into `out_shp`, with their index in `out_shx`.
/// Returns the 0-based numbers of the records kept, and how many there were.
fn cut_shp<R: Read>(mut shp: R, out_shp: &Path, out_shx: &Path, boxes: &[[f64; 4]]) -> io::Result<(Vec<u64>, u64)> {
    let mut header = [0u8; HEADER];
    shp.read_exact(&mut header)?;
    if header[..4] != 9994i32.to_be_bytes() {
        return Err(invalid("not a shapefile"));
    }
    let mut out = BufWriter::with_capacity(1 << 20, File::create(out_shp)?);
    let mut index = Vec::new();
    out.write_all(&header)?;
    let (mut kept, mut total, mut offset) = (Vec::new(), 0u64, HEADER as u64);
    let mut extent = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut record = Vec::new();
    let mut head = [0u8; 8];
    while read_full(&mut shp, &mut head)? {
        let words = i32::from_be_bytes(head[4..8].try_into().unwrap());
        let len = usize::try_from(words).map_err(|_| invalid("negative record length"))? * 2;
        record.resize(len, 0);
        shp.read_exact(&mut record)?;
        total += 1;
        // Every shape but the null one (type 0) starts with its type and its box.
        if len < 36 || i32::from_le_bytes(record[..4].try_into().unwrap()) == 0 {
            continue;
        }
        let f = |i: usize| f64::from_le_bytes(record[4 + 8 * i..12 + 8 * i].try_into().unwrap());
        let b = [f(0), f(1), f(2), f(3)];
        if !boxes.iter().any(|s| b[0] <= s[2] && s[0] <= b[2] && b[1] <= s[3] && s[1] <= b[3]) {
            continue;
        }
        for i in 0..2 {
            extent[i] = extent[i].min(b[i]);
            extent[i + 2] = extent[i + 2].max(b[i + 2]);
        }
        kept.push(total - 1);
        out.write_all(&(kept.len() as i32).to_be_bytes())?;
        out.write_all(&head[4..8])?;
        out.write_all(&record)?;
        index.extend_from_slice(&((offset / 2) as i32).to_be_bytes());
        index.extend_from_slice(&head[4..8]);
        offset += 8 + len as u64;
    }
    if kept.is_empty() {
        extent = [0.0; 4];
    }
    // The length of the file, in 16-bit words, and the box of what is kept.
    let fix = |out: &mut BufWriter<File>, words: u64| -> io::Result<()> {
        out.seek(SeekFrom::Start(24))?;
        out.write_all(&(i32::try_from(words).map_err(|_| invalid("the cut is too large"))?).to_be_bytes())?;
        out.seek(SeekFrom::Start(36))?;
        for v in extent {
            out.write_all(&v.to_le_bytes())?;
        }
        out.flush()
    };
    fix(&mut out, offset / 2)?;
    let mut shx = BufWriter::new(File::create(out_shx)?);
    shx.write_all(&header)?;
    shx.write_all(&index)?;
    fix(&mut shx, (HEADER + index.len()) as u64 / 2)?;
    Ok((kept, total))
}

/// Copies the attributes of the records `kept` (0-based, ascending) of `dbf`, which has `total`.
fn cut_dbf<R: Read>(mut dbf: R, out_dbf: &Path, kept: &[u64], total: u64) -> io::Result<()> {
    let mut start = [0u8; 32];
    dbf.read_exact(&mut start)?;
    let count = u64::from(u32::from_le_bytes(start[4..8].try_into().unwrap()));
    let header_len = usize::from(u16::from_le_bytes(start[8..10].try_into().unwrap()));
    let record_len = usize::from(u16::from_le_bytes(start[10..12].try_into().unwrap()));
    if count != total || header_len < 32 {
        return Err(invalid(&format!("{count} records of attributes for {total} shapes")));
    }
    let mut header = vec![0u8; header_len];
    header[..32].copy_from_slice(&start);
    dbf.read_exact(&mut header[32..])?;
    header[4..8].copy_from_slice(&(kept.len() as u32).to_le_bytes());
    let mut out = BufWriter::with_capacity(1 << 20, File::create(out_dbf)?);
    out.write_all(&header)?;
    let mut record = vec![0u8; record_len];
    let mut next = kept.iter().peekable();
    for i in 0..total {
        if next.peek().is_none() {
            break;
        }
        dbf.read_exact(&mut record)?;
        if next.peek() == Some(&&i) {
            next.next();
            out.write_all(&record)?;
        }
    }
    // The end-of-file marker of dBASE.
    out.write_all(&[0x1a])?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shapefile of polygons with these boxes (one ring, the box itself), as a zip.
    fn zip_of(boxes: &[[f64; 4]]) -> Vec<u8> {
        let mut shp = Vec::new();
        let mut shx = Vec::new();
        let mut header = vec![0u8; HEADER];
        header[..4].copy_from_slice(&9994i32.to_be_bytes());
        header[28..32].copy_from_slice(&1000i32.to_le_bytes());
        header[32..36].copy_from_slice(&5i32.to_le_bytes());
        shp.extend_from_slice(&header);
        shx.extend_from_slice(&header);
        for (n, b) in boxes.iter().enumerate() {
            let mut content = Vec::new();
            content.extend_from_slice(&5i32.to_le_bytes());
            b.iter().for_each(|v| content.extend_from_slice(&v.to_le_bytes()));
            content.extend_from_slice(&1i32.to_le_bytes());
            content.extend_from_slice(&5i32.to_le_bytes());
            content.extend_from_slice(&0i32.to_le_bytes());
            for (x, y) in [(b[0], b[1]), (b[0], b[3]), (b[2], b[3]), (b[2], b[1]), (b[0], b[1])] {
                content.extend_from_slice(&x.to_le_bytes());
                content.extend_from_slice(&y.to_le_bytes());
            }
            shx.extend_from_slice(&((shp.len() / 2) as i32).to_be_bytes());
            shx.extend_from_slice(&((content.len() / 2) as i32).to_be_bytes());
            shp.extend_from_slice(&(n as i32 + 1).to_be_bytes());
            shp.extend_from_slice(&((content.len() / 2) as i32).to_be_bytes());
            shp.extend_from_slice(&content);
        }
        let mut dbf = vec![3u8, 126, 1, 1];
        dbf.extend_from_slice(&(boxes.len() as u32).to_le_bytes());
        dbf.extend_from_slice(&65u16.to_le_bytes());
        dbf.extend_from_slice(&5u16.to_le_bytes());
        dbf.resize(32, 0);
        let mut field = b"ID".to_vec();
        field.resize(11, 0);
        field.push(b'N');
        field.extend_from_slice(&[0; 4]);
        field.push(4);
        field.resize(32, 0);
        dbf.extend_from_slice(&field);
        dbf.push(0x0d);
        for n in 0..boxes.len() {
            dbf.extend_from_slice(format!(" {n:>4}").as_bytes());
        }
        dbf.push(0x1a);
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, bytes) in [("shp", &shp), ("shx", &shx), ("dbf", &dbf), ("prj", &b"PROJCS".to_vec())] {
            zip.start_file(format!("polygons/water.{name}"), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn keeps_the_records_that_touch_the_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("water.zip");
        let boxes = [[0.0, 0.0, 10.0, 10.0], [20.0, 0.0, 30.0, 10.0], [5.0, 5.0, 25.0, 6.0], [40.0, 40.0, 50.0, 50.0]];
        std::fs::write(&zip, zip_of(&boxes)).unwrap();
        let cut = super::cut(&zip, tmp.path(), "water", &[[9.0, 1.0, 12.0, 2.0], [26.0, 9.0, 27.0, 20.0]]).unwrap();
        assert_eq!((cut.kept, cut.total), (2, 4));

        let shp = std::fs::read(tmp.path().join("water.shp")).unwrap();
        let shx = std::fs::read(tmp.path().join("water.shx")).unwrap();
        let dbf = std::fs::read(tmp.path().join("water.dbf")).unwrap();
        assert_eq!(i32::from_be_bytes(shp[24..28].try_into().unwrap()) as usize * 2, shp.len());
        assert_eq!(i32::from_be_bytes(shx[24..28].try_into().unwrap()) as usize * 2, shx.len());
        assert_eq!(shx.len(), HEADER + 2 * 8);
        let f = |i: usize| f64::from_le_bytes(shp[36 + 8 * i..44 + 8 * i].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2), f(3)], [0.0, 0.0, 30.0, 10.0]);
        // The second record, numbered 2, where the index says.
        let second = i32::from_be_bytes(shx[108..112].try_into().unwrap()) as usize * 2;
        assert_eq!(i32::from_be_bytes(shp[second..second + 4].try_into().unwrap()), 2);
        assert_eq!(f64::from_le_bytes(shp[second + 12..second + 20].try_into().unwrap()), 20.0);
        assert_eq!(u32::from_le_bytes(dbf[4..8].try_into().unwrap()), 2);
        assert_eq!(&dbf[65..], b"    0    1\x1a");
        assert_eq!(std::fs::read(tmp.path().join("water.prj")).unwrap(), b"PROJCS");

        let again = tempfile::tempdir().unwrap();
        super::cut(&zip, again.path(), "water", &[[9.0, 1.0, 12.0, 2.0], [26.0, 9.0, 27.0, 20.0]]).unwrap();
        assert_eq!(std::fs::read(again.path().join("water.shp")).unwrap(), shp);
    }
}
