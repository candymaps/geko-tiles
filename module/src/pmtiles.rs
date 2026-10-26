//! What the conversion reads of the PMTiles that Planetiler leaves (version 3 of
//! https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md): its header, its metadata and
//! every entry of its directories; and the tile ids of the specification (Hilbert order), which the
//! index of the bundle keeps.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use geko_module_sdk::{GekoError, Result};

pub const HEADER_LEN: usize = 127;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub root_offset: u64,
    pub root_len: u64,
    pub metadata_offset: u64,
    pub metadata_len: u64,
    pub leaves_offset: u64,
    pub leaves_len: u64,
    pub data_offset: u64,
    pub data_len: u64,
    /// 1 none, 2 gzip, 3 brotli, 4 zstd.
    pub internal_compression: u8,
    pub tile_compression: u8,
    /// 1 MVT, 6 MLT.
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
}

impl Header {
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < HEADER_LEN || &b[..7] != b"PMTiles" || b[7] != 3 {
            return Err(GekoError::Data("not a PMTiles archive of version 3".into()));
        }
        let u = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        Ok(Self {
            root_offset: u(8),
            root_len: u(16),
            metadata_offset: u(24),
            metadata_len: u(32),
            leaves_offset: u(40),
            leaves_len: u(48),
            data_offset: u(56),
            data_len: u(64),
            internal_compression: b[97],
            tile_compression: b[98],
            tile_type: b[99],
            min_zoom: b[100],
            max_zoom: b[101],
        })
    }
}

/// An entry of a directory: `run` tiles from `tile_id` with the same content, at `offset` (in the
/// tile data) with `len` bytes; with `run` 0, a leaf directory, at `offset` in the leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub len: u32,
    pub run: u32,
}

pub struct Archive {
    file: File,
    pub header: Header,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut b = [0u8; HEADER_LEN];
        file.read_exact(&mut b)?;
        let header = Header::parse(&b).map_err(|e| GekoError::Data(format!("{}: {e}", path.display())))?;
        if header.internal_compression != 2 && header.internal_compression != 1 {
            return Err(GekoError::Data(format!("{}: directories compressed with {}, not gzip", path.display(), header.internal_compression)));
        }
        Ok(Self { file, header })
    }

    fn read_at(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut bytes = vec![0u8; usize::try_from(len).map_err(|_| GekoError::Data("a part too large".into()))?];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn internal(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let bytes = self.read_at(offset, len)?;
        if self.header.internal_compression == 1 {
            return Ok(bytes);
        }
        let mut out = Vec::new();
        GzDecoder::new(&bytes[..]).read_to_end(&mut out)?;
        Ok(out)
    }

    /// The metadata, as JSON.
    pub fn metadata(&mut self) -> Result<Vec<u8>> {
        let (offset, len) = (self.header.metadata_offset, self.header.metadata_len);
        self.internal(offset, len)
    }

    /// Every entry of tiles (none of leaf directories), in order of tile id.
    pub fn entries(&mut self) -> Result<Vec<Entry>> {
        let (offset, len) = (self.header.root_offset, self.header.root_len);
        let mut out = Vec::new();
        self.collect(offset, len, &mut out, 0)?;
        Ok(out)
    }

    fn collect(&mut self, offset: u64, len: u64, out: &mut Vec<Entry>, depth: u32) -> Result<()> {
        if depth > 3 {
            return Err(GekoError::Data("PMTiles directories nested too deep".into()));
        }
        let bytes = self.internal(offset, len)?;
        for e in parse_directory(&bytes)? {
            if e.run == 0 {
                let leaves = self.header.leaves_offset;
                self.collect(leaves + e.offset, u64::from(e.len), out, depth + 1)?;
            } else {
                out.push(e);
            }
        }
        Ok(())
    }

    /// The bytes of a tile, from its entry.
    pub fn tile(&mut self, e: &Entry) -> Result<Vec<u8>> {
        let offset = self.header.data_offset + e.offset;
        self.read_at(offset, u64::from(e.len))
    }
}

fn varint(b: &[u8], at: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at).ok_or_else(|| GekoError::Data("a PMTiles directory ends too soon".into()))?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(v);
        }
    }
    Err(GekoError::Data("a varint too long in a PMTiles directory".into()))
}

pub fn parse_directory(b: &[u8]) -> Result<Vec<Entry>> {
    let mut at = 0;
    let n = usize::try_from(varint(b, &mut at)?).map_err(|_| GekoError::Data("too many entries".into()))?;
    if n > b.len() {
        return Err(GekoError::Data("a PMTiles directory with more entries than bytes".into()));
    }
    let mut entries = vec![Entry { tile_id: 0, offset: 0, len: 0, run: 0 }; n];
    let mut id = 0;
    for e in &mut entries {
        id += varint(b, &mut at)?;
        e.tile_id = id;
    }
    for e in &mut entries {
        e.run = u32::try_from(varint(b, &mut at)?).map_err(|_| GekoError::Data("a run too long".into()))?;
    }
    for e in &mut entries {
        e.len = u32::try_from(varint(b, &mut at)?).map_err(|_| GekoError::Data("a tile too large".into()))?;
    }
    for i in 0..n {
        let v = varint(b, &mut at)?;
        entries[i].offset = if v == 0 && i > 0 { entries[i - 1].offset + u64::from(entries[i - 1].len) } else { v.saturating_sub(1) };
    }
    Ok(entries)
}

fn rotate(n: u64, x: &mut u64, y: &mut u64, rx: u64, ry: u64) {
    if ry == 0 {
        if rx == 1 {
            *x = n - 1 - *x;
            *y = n - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

/// The id of a tile: the tiles of every lower zoom, then its place on the Hilbert curve of its zoom.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let base = ((1u64 << (2 * u32::from(z))) - 1) / 3;
    let n = 1u64 << z;
    let (mut x, mut y, mut d) = (u64::from(x), u64::from(y), 0u64);
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        rotate(n, &mut x, &mut y, rx, ry);
        s /= 2;
    }
    base + d
}

/// The tile of an id: `(z, x, y)`.
pub fn zxy(id: u64) -> (u8, u32, u32) {
    let mut base = 0u64;
    for z in 0..32u8 {
        let count = 1u64 << (2 * u32::from(z));
        if id < base + count {
            let n = 1u64 << z;
            let (mut x, mut y, mut t) = (0u64, 0u64, id - base);
            let mut s = 1;
            while s < n {
                let rx = 1 & (t / 2);
                let ry = 1 & (t ^ rx);
                rotate(s, &mut x, &mut y, rx, ry);
                x += s * rx;
                y += s * ry;
                t /= 4;
                s *= 2;
            }
            return (z, x as u32, y as u32);
        }
        base += count;
    }
    unreachable!("a tile id beyond zoom 31")
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tile_ids_of_the_specification() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
        assert_eq!(tile_id(12, 3423, 1763), 19_078_479);
        for id in [0, 1, 4, 5, 20, 21, 1_365, 19_078_479, 357_913_941_u64 + 12_345] {
            let (z, x, y) = zxy(id);
            assert_eq!(tile_id(z, x, y), id);
        }
    }

    fn push(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn gzip(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    pub fn directory(entries: &[Entry]) -> Vec<u8> {
        let mut out = Vec::new();
        push(&mut out, entries.len() as u64);
        let mut last = 0;
        for e in entries {
            push(&mut out, e.tile_id - last);
            last = e.tile_id;
        }
        entries.iter().for_each(|e| push(&mut out, u64::from(e.run)));
        entries.iter().for_each(|e| push(&mut out, u64::from(e.len)));
        for (i, e) in entries.iter().enumerate() {
            let next = i > 0 && e.offset == entries[i - 1].offset + u64::from(entries[i - 1].len);
            push(&mut out, if next { 0 } else { e.offset + 1 });
        }
        out
    }

    /// A PMTiles of `tiles` (id, content), in that order, with one leaf directory for those from
    /// `leaf_from`, and identical contents stored once.
    pub fn archive(tiles: &[(u64, &[u8])], leaf_from: usize) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        for (id, bytes) in tiles {
            let offset = match tiles.iter().position(|(_, b)| b == bytes) {
                Some(first) if tiles[first].0 != *id => entries[first].offset,
                _ => {
                    data.extend_from_slice(bytes);
                    (data.len() - bytes.len()) as u64
                }
            };
            entries.push(Entry { tile_id: *id, offset, len: bytes.len() as u32, run: 1 });
        }
        let leaf = gzip(&directory(&entries[leaf_from..]));
        let mut root = entries[..leaf_from].to_vec();
        root.push(Entry { tile_id: entries[leaf_from].tile_id, offset: 0, len: leaf.len() as u32, run: 0 });
        let root = gzip(&directory(&root));
        let metadata = gzip(br#"{"name":"test"}"#);
        let mut h = vec![0u8; HEADER_LEN];
        h[..7].copy_from_slice(b"PMTiles");
        h[7] = 3;
        let mut at = HEADER_LEN as u64;
        for (field, len) in [(8, root.len()), (24, metadata.len()), (40, leaf.len()), (56, data.len())] {
            h[field..field + 8].copy_from_slice(&at.to_le_bytes());
            h[field + 8..field + 16].copy_from_slice(&(len as u64).to_le_bytes());
            at += len as u64;
        }
        (h[97], h[98], h[99], h[100], h[101]) = (2, 2, 1, 0, 14);
        [h, root, metadata, leaf, data].concat()
    }

    #[test]
    fn reads_entries_through_leaves_and_tiles() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.pmtiles");
        let tiles: [(u64, &[u8]); 4] = [(0, b"world"), (1, b"sea"), (2, b"land"), (5, b"sea")];
        std::fs::write(&path, archive(&tiles, 2)).unwrap();
        let mut a = Archive::open(&path).unwrap();
        assert_eq!((a.header.tile_type, a.header.max_zoom), (1, 14));
        assert_eq!(a.metadata().unwrap(), br#"{"name":"test"}"#);
        let entries = a.entries().unwrap();
        assert_eq!(entries.iter().map(|e| e.tile_id).collect::<Vec<_>>(), [0, 1, 2, 5]);
        assert_eq!(entries[1].offset, entries[3].offset);
        for (e, (_, bytes)) in entries.iter().zip(tiles) {
            assert_eq!(a.tile(e).unwrap(), bytes);
        }
    }
}
