//! The merge of the OSM extracts of the scope into one PBF (docs/MODULE_PLAN.md, «Varios
//! extractos»), for a single run of Planetiler. Geofabrik publishes each extract sorted by type and
//! then by id, so the merge reads them all at once, in that order, and writes each object once: the
//! copy of the first extract, by id, that has it (the extracts of a day are the same snapshot).
//! Only what Planetiler reads is written (positions, tags, references and members, not the
//! versions or timestamps of the objects), and blocks are filled and compressed always the same
//! way, so the same extracts give the same bytes.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use geko_module_sdk::{GekoError, Result};
use osmpbf::{BlobDecode, BlobReader, Element, RelMemberType};

use crate::geo::BBox;

/// Objects per block, as osmium writes them.
const BLOCK: usize = 8000;

/// The box in the header of a PBF, if it has one.
pub fn header_bbox(path: &Path) -> Result<Option<BBox>> {
    let error = |e: osmpbf::Error| GekoError::Data(format!("{}: {e}", path.display()));
    let mut reader = BlobReader::from_path(path).map_err(error)?;
    let Some(blob) = reader.next() else { return Ok(None) };
    match blob.map_err(error)?.decode().map_err(error)? {
        BlobDecode::OsmHeader(header) => Ok(header.bbox().map(|b| BBox { west: b.left, south: b.bottom, east: b.right, north: b.top })),
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Latitude and longitude, in 1e-7 degrees.
    Node { lat: i32, lon: i32 },
    Way { refs: Vec<i64> },
    /// Each member: its type (0 node, 1 way, 2 relation), id and role.
    Relation { members: Vec<(u8, i64, String)> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub id: i64,
    pub tags: Vec<(String, String)>,
    pub body: Body,
}

impl Entity {
    fn kind(&self) -> u8 {
        match self.body {
            Body::Node { .. } => 0,
            Body::Way { .. } => 1,
            Body::Relation { .. } => 2,
        }
    }

    /// Its place in a file sorted by type and then by id.
    fn order(&self) -> (u8, i64) {
        (self.kind(), self.id)
    }
}

fn tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(String, String)> {
    tags.map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

/// The objects of a PBF, a block at a time.
struct Stream {
    path: String,
    reader: BlobReader<BufReader<File>>,
    pending: VecDeque<Entity>,
    last: Option<(u8, i64)>,
}

impl Stream {
    fn open(path: &Path) -> Result<Self> {
        let reader = BlobReader::from_path(path).map_err(|e| GekoError::Data(format!("{}: {e}", path.display())))?;
        Ok(Self { path: path.display().to_string(), reader, pending: VecDeque::new(), last: None })
    }

    fn peek(&mut self) -> Result<Option<&Entity>> {
        while self.pending.is_empty() {
            let Some(blob) = self.reader.next() else { return Ok(None) };
            let error = |e: osmpbf::Error| GekoError::Data(format!("{}: {e}", self.path));
            let BlobDecode::OsmData(block) = blob.map_err(error)?.decode().map_err(error)? else { continue };
            for element in block.elements() {
                let entity = match element {
                    Element::Node(n) => Entity { id: n.id(), tags: tags(n.tags()), body: Body::Node { lat: n.decimicro_lat(), lon: n.decimicro_lon() } },
                    Element::DenseNode(n) => Entity { id: n.id(), tags: tags(n.tags()), body: Body::Node { lat: n.decimicro_lat(), lon: n.decimicro_lon() } },
                    Element::Way(w) => Entity { id: w.id(), tags: tags(w.tags()), body: Body::Way { refs: w.refs().collect() } },
                    Element::Relation(r) => {
                        let members = r
                            .members()
                            .map(|m| {
                                let kind = match m.member_type {
                                    RelMemberType::Node => 0,
                                    RelMemberType::Way => 1,
                                    RelMemberType::Relation => 2,
                                };
                                Ok((kind, m.member_id, m.role().map_err(error)?.to_owned()))
                            })
                            .collect::<Result<_>>()?;
                        Entity { id: r.id(), tags: tags(r.tags()), body: Body::Relation { members } }
                    }
                };
                if self.last.is_some_and(|last| entity.order() <= last) {
                    return Err(GekoError::Data(format!("{}: not sorted by type and id (at {:?} {})", self.path, entity.kind(), entity.id)));
                }
                self.last = Some(entity.order());
                self.pending.push_back(entity);
            }
        }
        Ok(self.pending.front())
    }

    fn pop(&mut self) -> Option<Entity> {
        self.pending.pop_front()
    }
}

pub struct Merged {
    pub objects: u64,
    /// Objects found in more than one extract, written once.
    pub repeated: u64,
}

/// Merges `inputs` into `output`, with `bbox` in its header.
pub fn merge(inputs: &[&Path], output: &Path, bbox: Option<BBox>) -> Result<Merged> {
    let mut streams = inputs.iter().map(|p| Stream::open(p)).collect::<Result<Vec<_>>>()?;
    let mut writer = Writer::create(output, bbox)?;
    let (mut objects, mut repeated) = (0, 0);
    loop {
        let mut next: Option<(usize, (u8, i64))> = None;
        for (i, s) in streams.iter_mut().enumerate() {
            if let Some(e) = s.peek()?
                && next.is_none_or(|(_, order)| e.order() < order)
            {
                next = Some((i, e.order()));
            }
        }
        let Some((first, order)) = next else { break };
        let entity = streams[first].pop().expect("peeked");
        for s in &mut streams[first + 1..] {
            if s.peek()?.is_some_and(|e| e.order() == order) {
                s.pop();
                repeated += 1;
            }
        }
        writer.add(entity)?;
        objects += 1;
    }
    writer.finish()?;
    Ok(Merged { objects, repeated })
}

/// Writes a PBF: its header, then blocks of up to `BLOCK` objects of one type, compressed with zlib.
pub struct Writer {
    out: BufWriter<File>,
    block: Vec<Entity>,
}

impl Writer {
    pub fn create(path: &Path, bbox: Option<BBox>) -> Result<Self> {
        let mut writer = Self { out: BufWriter::with_capacity(1 << 20, File::create(path)?), block: Vec::new() };
        let mut header = Vec::new();
        if let Some(b) = bbox {
            let nano = |deg: f64| (deg * 1e9).round() as i64;
            let mut m = Vec::new();
            for (field, value) in [(1, b.west), (2, b.east), (3, b.north), (4, b.south)] {
                sint_field(&mut m, field, nano(value));
            }
            bytes_field(&mut header, 1, &m);
        }
        for feature in ["OsmSchema-V0.6", "DenseNodes"] {
            bytes_field(&mut header, 4, feature.as_bytes());
        }
        bytes_field(&mut header, 5, b"Sort.Type_then_ID");
        bytes_field(&mut header, 16, b"geko-tiles");
        writer.blob("OSMHeader", &header)?;
        Ok(writer)
    }

    pub fn add(&mut self, entity: Entity) -> Result<()> {
        if self.block.len() == BLOCK || self.block.last().is_some_and(|e| e.kind() != entity.kind()) {
            self.flush_block()?;
        }
        self.block.push(entity);
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.flush_block()?;
        self.out.flush()?;
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let block = encode_block(&self.block);
        self.block.clear();
        self.blob("OSMData", &block)
    }

    fn blob(&mut self, kind: &str, raw: &[u8]) -> Result<()> {
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(raw)?;
        let mut blob = Vec::new();
        int_field(&mut blob, 2, raw.len() as u64);
        bytes_field(&mut blob, 3, &zlib.finish()?);
        let mut header = Vec::new();
        bytes_field(&mut header, 1, kind.as_bytes());
        int_field(&mut header, 3, blob.len() as u64);
        self.out.write_all(&(header.len() as u32).to_be_bytes())?;
        self.out.write_all(&header)?;
        self.out.write_all(&blob)?;
        Ok(())
    }
}

/// A `PrimitiveBlock` of objects of one type: its string table (the empty string first, then each
/// string where it first appears) and one group.
fn encode_block(entities: &[Entity]) -> Vec<u8> {
    let mut strings = Strings::default();
    let mut group = Vec::new();
    match entities[0].body {
        Body::Node { .. } => {
            let (mut ids, mut lats, mut lons, mut keys_vals) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let (mut id0, mut lat0, mut lon0) = (0i64, 0i64, 0i64);
            for e in entities {
                let Body::Node { lat, lon } = e.body else { unreachable!("one type per block") };
                push_sint(&mut ids, e.id - id0);
                push_sint(&mut lats, i64::from(lat) - lat0);
                push_sint(&mut lons, i64::from(lon) - lon0);
                (id0, lat0, lon0) = (e.id, i64::from(lat), i64::from(lon));
                for (k, v) in &e.tags {
                    push_varint(&mut keys_vals, u64::from(strings.index(k)));
                    push_varint(&mut keys_vals, u64::from(strings.index(v)));
                }
                push_varint(&mut keys_vals, 0);
            }
            let mut dense = Vec::new();
            bytes_field(&mut dense, 1, &ids);
            bytes_field(&mut dense, 8, &lats);
            bytes_field(&mut dense, 9, &lons);
            bytes_field(&mut dense, 10, &keys_vals);
            bytes_field(&mut group, 2, &dense);
        }
        Body::Way { .. } => {
            for e in entities {
                let Body::Way { refs } = &e.body else { unreachable!("one type per block") };
                let mut way = Vec::new();
                int_field(&mut way, 1, e.id as u64);
                tags_fields(&mut way, &e.tags, &mut strings);
                let mut packed = Vec::new();
                let mut previous = 0;
                for &r in refs {
                    push_sint(&mut packed, r - previous);
                    previous = r;
                }
                bytes_field(&mut way, 8, &packed);
                bytes_field(&mut group, 3, &way);
            }
        }
        Body::Relation { .. } => {
            for e in entities {
                let Body::Relation { members } = &e.body else { unreachable!("one type per block") };
                let mut relation = Vec::new();
                int_field(&mut relation, 1, e.id as u64);
                tags_fields(&mut relation, &e.tags, &mut strings);
                let (mut roles, mut ids, mut types) = (Vec::new(), Vec::new(), Vec::new());
                let mut previous = 0;
                for (kind, id, role) in members {
                    push_varint(&mut roles, u64::from(strings.index(role)));
                    push_sint(&mut ids, id - previous);
                    previous = *id;
                    push_varint(&mut types, u64::from(*kind));
                }
                bytes_field(&mut relation, 8, &roles);
                bytes_field(&mut relation, 9, &ids);
                bytes_field(&mut relation, 10, &types);
                bytes_field(&mut group, 4, &relation);
            }
        }
    }
    let mut table = Vec::new();
    for s in &strings.list {
        bytes_field(&mut table, 1, s.as_bytes());
    }
    let mut block = Vec::new();
    bytes_field(&mut block, 1, &table);
    bytes_field(&mut block, 2, &group);
    block
}

fn tags_fields(out: &mut Vec<u8>, tags: &[(String, String)], strings: &mut Strings) {
    let (mut keys, mut vals) = (Vec::new(), Vec::new());
    for (k, v) in tags {
        push_varint(&mut keys, u64::from(strings.index(k)));
        push_varint(&mut vals, u64::from(strings.index(v)));
    }
    if !tags.is_empty() {
        bytes_field(out, 2, &keys);
        bytes_field(out, 3, &vals);
    }
}

struct Strings {
    list: Vec<String>,
    index: HashMap<String, u32>,
}

impl Default for Strings {
    fn default() -> Self {
        Self { list: vec![String::new()], index: HashMap::from([(String::new(), 0)]) }
    }
}

impl Strings {
    fn index(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.to_owned());
        self.index.insert(s.to_owned(), i);
        i
    }
}

// Protocol buffers, by hand: only what a PBF needs.

fn push_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn push_sint(out: &mut Vec<u8>, v: i64) {
    push_varint(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn int_field(out: &mut Vec<u8>, field: u32, v: u64) {
    push_varint(out, u64::from(field) << 3);
    push_varint(out, v);
}

fn sint_field(out: &mut Vec<u8>, field: u32, v: i64) {
    push_varint(out, u64::from(field) << 3);
    push_sint(out, v);
}

fn bytes_field(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    push_varint(out, (u64::from(field) << 3) | 2);
    push_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, lat: i32, tags: &[(&str, &str)]) -> Entity {
        Entity { id, tags: tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body: Body::Node { lat, lon: -lat } }
    }

    fn way(id: i64, refs: &[i64]) -> Entity {
        Entity { id, tags: vec![("highway".into(), "primary".into())], body: Body::Way { refs: refs.to_vec() } }
    }

    fn relation(id: i64) -> Entity {
        let members = vec![(1, 20, "outer".into()), (0, 1, String::new()), (2, 31, "subarea".into())];
        Entity { id, tags: vec![("type".into(), "multipolygon".into())], body: Body::Relation { members } }
    }

    fn write(path: &Path, entities: &[Entity], bbox: Option<BBox>) {
        let mut w = Writer::create(path, bbox).unwrap();
        for e in entities {
            w.add(e.clone()).unwrap();
        }
        w.finish().unwrap();
    }

    fn read(path: &Path) -> Vec<Entity> {
        let mut s = Stream::open(path).unwrap();
        let mut out = Vec::new();
        while s.peek().unwrap().is_some() {
            out.push(s.pop().unwrap());
        }
        out
    }

    #[test]
    fn what_is_written_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.osm.pbf");
        // More nodes than a block, so that the deltas start again in the next one.
        let mut entities: Vec<Entity> = (1..=BLOCK as i64 + 5).map(|id| node(id, 395_000_000 + id as i32, &[])).collect();
        entities[3] = node(4, -100, &[("name", "Palma"), ("place", "city")]);
        entities.extend([way(20, &[1, 2, 3, 1]), way(21, &[5, 4]), relation(30), relation(31)]);
        let bbox = BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 };
        write(&path, &entities, Some(bbox));
        assert_eq!(read(&path), entities);
        let read = header_bbox(&path).unwrap().unwrap();
        for (a, b) in [(read.west, bbox.west), (read.south, bbox.south), (read.east, bbox.east), (read.north, bbox.north)] {
            assert!((a - b).abs() < 1e-9, "{read:?}");
        }
    }

    #[test]
    fn merges_in_order_and_writes_each_object_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, out) = (tmp.path().join("a.pbf"), tmp.path().join("b.pbf"), tmp.path().join("out.pbf"));
        write(&a, &[node(1, 10, &[]), node(3, 30, &[("a", "1")]), way(20, &[1, 3]), relation(30)], None);
        write(&b, &[node(2, 20, &[]), node(3, 30, &[("a", "1")]), node(4, 40, &[]), way(20, &[1, 3]), way(21, &[3, 4])], None);
        let merged = merge(&[&a, &b], &out, None).unwrap();
        assert_eq!((merged.objects, merged.repeated), (7, 2));
        let ids: Vec<(u8, i64)> = read(&out).iter().map(Entity::order).collect();
        assert_eq!(ids, [(0, 1), (0, 2), (0, 3), (0, 4), (1, 20), (1, 21), (2, 30)]);
        assert_eq!(header_bbox(&out).unwrap(), None);

        let again = tmp.path().join("again.pbf");
        merge(&[&a, &b], &again, None).unwrap();
        assert_eq!(std::fs::read(&again).unwrap(), std::fs::read(&out).unwrap());
    }

    #[test]
    fn refuses_an_extract_out_of_order() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, out) = (tmp.path().join("a.pbf"), tmp.path().join("out.pbf"));
        write(&a, &[node(2, 0, &[]), node(1, 0, &[])], None);
        let err = merge(&[&a], &out, None).err().unwrap().to_string();
        assert!(err.contains("not sorted"), "{err}");
    }
}
