//! The extent of the scope: the boxes of its OSM extracts, and the tiles that touch them, which are
//! what the osmdata cut keeps and what the conversion keeps from `SCOPE_ZOOM` up.

use serde::{Deserialize, Serialize};

/// From this zoom up, only the tiles that touch the scope; below it, the whole world (the context of
/// Natural Earth, 1,365 tiles).
pub const SCOPE_ZOOM: u8 = 6;

/// The zoom of the deepest tiles (docs/MODULE_PLAN.md, «Medidas»: 14 is half the size of 15).
pub const MAX_ZOOM: u8 = 14;

/// Longitude and latitude, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// The latitude that Web Mercator stops at.
const MAX_LAT: f64 = 85.051_128_779_806_59;
const EARTH_RADIUS: f64 = 6_378_137.0;

impl BBox {
    /// The range of tiles of `zoom` that touch it, inclusive: `(x0, y0, x1, y1)`.
    pub fn tiles(&self, zoom: u8) -> (u32, u32, u32, u32) {
        let n = 1u32 << zoom;
        let x = |lon: f64| ((((lon + 180.0) / 360.0) * f64::from(n)).floor().max(0.0) as u32).min(n - 1);
        let y = |lat: f64| {
            let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
            let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * f64::from(n);
            (y.floor().max(0.0) as u32).min(n - 1)
        };
        (x(self.west), y(self.north), x(self.east), y(self.south))
    }

    /// The tiles of `SCOPE_ZOOM` that touch it, as one box, a little wider (the buffer of a tile):
    /// what the osmdata cut keeps.
    pub fn scope_tiles(&self) -> BBox {
        let (x0, y0, x1, y1) = self.tiles(SCOPE_ZOOM);
        let (b0, b1) = (tile_bbox(SCOPE_ZOOM, x0, y0), tile_bbox(SCOPE_ZOOM, x1, y1));
        let margin = (b0.east - b0.west) / 32.0;
        BBox { west: b0.west - margin, south: b1.south - margin, east: b1.east + margin, north: b0.north + margin }
    }

    /// In Web Mercator (EPSG:3857) metres: `[xmin, ymin, xmax, ymax]`.
    pub fn mercator(&self) -> [f64; 4] {
        let x = |lon: f64| EARTH_RADIUS * lon.clamp(-180.0, 180.0).to_radians();
        let y = |lat: f64| EARTH_RADIUS * (std::f64::consts::FRAC_PI_4 + lat.clamp(-MAX_LAT, MAX_LAT).to_radians() / 2.0).tan().ln();
        [x(self.west), y(self.south), x(self.east), y(self.north)]
    }
}

pub fn tile_bbox(zoom: u8, x: u32, y: u32) -> BBox {
    let n = f64::from(1u32 << zoom);
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    BBox { west: lon(f64::from(x)), south: lat(f64::from(y) + 1.0), east: lon(f64::from(x) + 1.0), north: lat(f64::from(y)) }
}

/// Whether the tile is kept: every tile below `SCOPE_ZOOM`; from it up, those that touch the scope.
pub fn keep(scope: &[BBox], zoom: u8, x: u32, y: u32) -> bool {
    if zoom < SCOPE_ZOOM {
        return true;
    }
    scope.iter().any(|b| {
        let (x0, y0, x1, y1) = b.tiles(zoom);
        (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALEARES: BBox = BBox { west: 1.0, south: 38.5, east: 4.6, north: 40.2 };

    #[test]
    fn tiles_of_a_box() {
        // z6: 5.625° wide; 1° to 4.6° is column 32, and row 24 spans 34.3° to 40.98°.
        assert_eq!(BALEARES.tiles(6), (32, 24, 32, 24));
        assert_eq!(BALEARES.tiles(7), (64, 48, 65, 49));
        assert_eq!(BALEARES.tiles(0), (0, 0, 0, 0));
        let b = tile_bbox(6, 32, 24);
        assert!((b.west - 0.0).abs() < 1e-9 && (b.east - 5.625).abs() < 1e-9 && b.north > 40.9 && b.south < 40.0);
        let world = BBox { west: -180.0, south: -90.0, east: 180.0, north: 90.0 };
        assert_eq!(world.tiles(2), (0, 0, 3, 3));
    }

    #[test]
    fn the_cut_covers_every_kept_tile() {
        let cut = BALEARES.scope_tiles();
        for zoom in SCOPE_ZOOM..=MAX_ZOOM {
            let (x0, y0, x1, y1) = BALEARES.tiles(zoom);
            for (x, y) in [(x0, y0), (x1, y1)] {
                let t = tile_bbox(zoom, x, y);
                assert!(t.west >= cut.west && t.east <= cut.east && t.south >= cut.south && t.north <= cut.north, "z{zoom}");
                assert!(keep(&[BALEARES], zoom, x, y));
            }
            assert!(!keep(&[BALEARES], zoom, x1 + 1, y0));
        }
        assert!(keep(&[BALEARES], 5, 0, 0));
    }

    #[test]
    fn web_mercator() {
        let [x0, y0, x1, y1] = BBox { west: -180.0, south: -MAX_LAT, east: 180.0, north: MAX_LAT }.mercator();
        assert!((x1 - 20_037_508.342_789_244).abs() < 1e-6 && (x0 + x1).abs() < 1e-6);
        assert!((y1 - 20_037_508.342_789_244).abs() < 1e-3 && (y0 + y1).abs() < 1e-3);
    }
}
