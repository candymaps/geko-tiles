//! What the module has answered, for `stats.json`: requests, errors and time of every route, the
//! tiles of each zoom level, the last hour minute by minute and the styles asked for. The host
//! measures requests per route for `/metrics`, but a module cannot read it; these are its own
//! counters, a few atomics per request and one short lock for the minute it falls in.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Upper bounds of the time buckets, in microseconds: a tile takes a few, so they start at one.
const BOUNDS_US: [u64; 11] = [1, 2, 5, 10, 25, 50, 100, 250, 1_000, 5_000, 25_000];
const BUCKETS: usize = BOUNDS_US.len() + 1;
const MINUTES: usize = 60;
const SECONDS: usize = 60;
const ZOOMS: usize = 32;

/// The routes that are counted, as `Request::route` names them.
const ROUTES: [&str; 6] = ["{z}/{x}/{y}", "tilejson.json", "styles.json", "styles/{flavor}/{lang}", "fonts/{fontstack}/{range}", "sprites/{name}"];

fn bucket(us: u64) -> usize {
    BOUNDS_US.iter().position(|&b| us <= b).unwrap_or(BOUNDS_US.len())
}

/// The upper bound, in microseconds, of the bucket that holds the `q` quantile of the counts: the
/// real value is at most that. `None` without counts, or when it falls past the last bound.
fn quantile_us(counts: &[u64; BUCKETS], q: f64) -> Option<u64> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return None;
    }
    let wanted = (total as f64 * q).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (i, &n) in counts.iter().enumerate() {
        seen += n;
        if seen >= wanted {
            return BOUNDS_US.get(i).copied();
        }
    }
    None
}

#[derive(Default)]
struct Route {
    requests: AtomicU64,
    errors: AtomicU64,
    time: [AtomicU64; BUCKETS],
}

#[derive(Default)]
struct Zoom {
    tiles: AtomicU64,
    empty: AtomicU64,
    bytes: AtomicU64,
}

/// A minute of the last hour, or a second of the last minute. Lock-free: a tile takes microseconds
/// and many threads serve them, so a lock here would cost more than the tile. The thread that finds
/// the slot of an older period starts it over; the few requests of the same instant that land
/// before the reset are lost.
#[derive(Default)]
struct Minute {
    /// Minutes (seconds, in the ring of seconds) since the epoch.
    at: AtomicU64,
    requests: AtomicU64,
    time: [AtomicU64; BUCKETS],
}

impl Minute {
    fn add(&self, at: u64, bucket: usize) {
        let seen = self.at.load(Ordering::Relaxed);
        if seen != at && self.at.compare_exchange(seen, at, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            self.requests.store(0, Ordering::Relaxed);
            self.time.iter().for_each(|t| t.store(0, Ordering::Relaxed));
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.time[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Requests and time counts if the slot holds `at`, zero otherwise.
    fn read(&self, at: u64) -> (u64, [u64; BUCKETS]) {
        if self.at.load(Ordering::Relaxed) == at {
            (self.requests.load(Ordering::Relaxed), std::array::from_fn(|i| self.time[i].load(Ordering::Relaxed)))
        } else {
            (0, [0; BUCKETS])
        }
    }
}

/// The slots that end at `now`, oldest first.
fn window<const N: usize>(ring: &[Minute; N], now: u64) -> Vec<(u64, [u64; BUCKETS])> {
    (0..N as u64).map(|i| {
        let at = now.saturating_sub(N as u64 - 1 - i);
        ring[(at % N as u64) as usize].read(at)
    }).collect()
}

pub struct Stats {
    started: u64,
    routes: [Route; ROUTES.len()],
    zooms: [Zoom; ZOOMS],
    minutes: [Minute; MINUTES],
    seconds: [Minute; SECONDS],
    /// Styles asked for and answered, as `flavor/language`: at most a few hundred keys.
    styles: Mutex<HashMap<String, u64>>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            started: now_secs(),
            routes: Default::default(),
            zooms: std::array::from_fn(|_| Zoom::default()),
            minutes: std::array::from_fn(|_| Minute::default()),
            seconds: std::array::from_fn(|_| Minute::default()),
            styles: Mutex::new(HashMap::new()),
        }
    }
}

impl Stats {
    pub fn record(&self, route: &str, status: u16, micros: u64) {
        self.record_at(now_secs(), route, status, micros);
    }

    fn record_at(&self, second: u64, route: &str, status: u16, micros: u64) {
        let Some(i) = ROUTES.iter().position(|r| *r == route) else { return };
        let b = bucket(micros);
        let r = &self.routes[i];
        r.requests.fetch_add(1, Ordering::Relaxed);
        if status >= 400 {
            r.errors.fetch_add(1, Ordering::Relaxed);
        }
        r.time[b].fetch_add(1, Ordering::Relaxed);
        let minute = second / 60;
        self.minutes[(minute % MINUTES as u64) as usize].add(minute, b);
        self.seconds[(second % SECONDS as u64) as usize].add(second, b);
    }

    /// A tile of zoom `z` answered: `bytes` is its size, 0 when there was none (204).
    pub fn tile(&self, z: u8, bytes: usize) {
        let zoom = &self.zooms[usize::from(z).min(ZOOMS - 1)];
        if bytes == 0 {
            zoom.empty.fetch_add(1, Ordering::Relaxed);
        } else {
            zoom.tiles.fetch_add(1, Ordering::Relaxed);
            zoom.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }

    pub fn style(&self, flavor: &str, lang: &str) {
        *self.styles.lock().unwrap_or_else(|e| e.into_inner()).entry(format!("{flavor}/{lang}")).or_default() += 1;
    }

    pub fn snapshot(&self) -> Value {
        self.snapshot_at(now_secs())
    }

    fn snapshot_at(&self, now_second: u64) -> Value {
        let now = now_second / 60;
        let routes: Vec<Value> = ROUTES
            .iter()
            .zip(&self.routes)
            .map(|(name, r)| {
                let counts: [u64; BUCKETS] = std::array::from_fn(|i| r.time[i].load(Ordering::Relaxed));
                json!({
                    "route": name,
                    "requests": r.requests.load(Ordering::Relaxed),
                    "errors": r.errors.load(Ordering::Relaxed),
                    "p50_us": quantile_us(&counts, 0.5),
                    "p95_us": quantile_us(&counts, 0.95),
                    "p99_us": quantile_us(&counts, 0.99),
                })
            })
            .collect();
        let zooms: Vec<Value> = self
            .zooms
            .iter()
            .enumerate()
            .filter_map(|(z, c)| {
                let (tiles, empty) = (c.tiles.load(Ordering::Relaxed), c.empty.load(Ordering::Relaxed));
                (tiles + empty > 0).then(|| json!({ "zoom": z, "tiles": tiles, "empty": empty, "bytes": c.bytes.load(Ordering::Relaxed) }))
            })
            .collect();
        let mut styles: Vec<(String, u64)> = self.styles.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), *v)).collect();
        styles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        styles.truncate(10);
        // The last hour, oldest first; a minute with no requests counts zero.
        let series = window(&self.minutes, now);
        let recent = window(&self.seconds, now_second);
        json!({
            "since_unix": self.started,
            "routes": routes,
            "zooms": zooms,
            "styles": styles.iter().map(|(name, requests)| json!({ "style": name, "requests": requests })).collect::<Vec<_>>(),
            "last_hour": {
                "end_unix": (now + 1) * 60,
                "requests": series.iter().map(|m| m.0).collect::<Vec<_>>(),
                "p99_us": series.iter().map(|m| quantile_us(&m.1, 0.99)).collect::<Vec<_>>(),
            },
            "last_minute": {
                "end_unix": now_second + 1,
                "requests": recent.iter().map(|m| m.0).collect::<Vec<_>>(),
                "p99_us": recent.iter().map(|m| quantile_us(&m.1, 0.99)).collect::<Vec<_>>(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_are_the_bound_of_their_bucket() {
        let mut counts = [0; BUCKETS];
        assert_eq!(quantile_us(&counts, 0.5), None);
        counts[bucket(2)] = 90;
        counts[bucket(8)] = 9;
        counts[bucket(40)] = 1;
        assert_eq!(quantile_us(&counts, 0.5), Some(2));
        assert_eq!(quantile_us(&counts, 0.95), Some(10));
        assert_eq!(quantile_us(&counts, 1.0), Some(50));
        counts[bucket(900_000)] = 100;
        assert_eq!(quantile_us(&counts, 1.0), None, "past the last bound");
    }

    #[test]
    fn counts_routes_errors_zooms_styles_and_the_last_hour() {
        let stats = Stats::default();
        stats.record_at(60_000, "{z}/{x}/{y}", 200, 3);
        stats.record_at(60_000, "{z}/{x}/{y}", 404, 4);
        stats.record_at(60_600, "tilejson.json", 200, 2);
        stats.record_at(60_600, "nope", 200, 2);
        stats.tile(14, 120);
        stats.tile(14, 0);
        stats.tile(99, 10);
        stats.style("light", "en");
        stats.style("light", "en");
        stats.style("dark", "es");
        let s = stats.snapshot_at(60_600);
        let tile = &s["routes"][0];
        assert_eq!((tile["requests"].as_u64(), tile["errors"].as_u64(), tile["p99_us"].as_u64()), (Some(2), Some(1), Some(5)));
        let z14 = &s["zooms"][0];
        assert_eq!((z14["zoom"].as_u64(), z14["tiles"].as_u64(), z14["empty"].as_u64(), z14["bytes"].as_u64()), (Some(14), Some(1), Some(1), Some(120)));
        assert_eq!(s["zooms"][1]["zoom"].as_u64(), Some(31), "a zoom past the last one counts in it");
        assert_eq!((s["styles"][0]["style"].as_str(), s["styles"][0]["requests"].as_u64()), (Some("light/en"), Some(2)));
        let hour = s["last_hour"]["requests"].as_array().unwrap();
        assert_eq!(hour.len(), MINUTES);
        assert_eq!((hour[MINUTES - 1].as_u64(), hour[MINUTES - 11].as_u64(), hour[0].as_u64()), (Some(1), Some(2), Some(0)));
    }

    #[test]
    fn a_minute_an_hour_old_starts_over() {
        let stats = Stats::default();
        stats.record_at(60_000, "tilejson.json", 200, 3);
        stats.record_at(60_000 + 60 * MINUTES as u64, "tilejson.json", 200, 3);
        let s = stats.snapshot_at(60_000 + 60 * MINUTES as u64);
        let total: u64 = s["last_hour"]["requests"].as_array().unwrap().iter().filter_map(Value::as_u64).sum();
        assert_eq!(total, 1);
    }

    #[test]
    fn the_last_minute_goes_second_by_second() {
        let stats = Stats::default();
        stats.record_at(5_000, "tilejson.json", 200, 3);
        stats.record_at(5_058, "tilejson.json", 200, 3);
        stats.record_at(5_058, "tilejson.json", 200, 3);
        stats.record_at(5_059, "tilejson.json", 200, 3);
        let s = stats.snapshot_at(5_059);
        let secs = s["last_minute"]["requests"].as_array().unwrap();
        assert_eq!(secs.len(), SECONDS);
        assert_eq!((secs[SECONDS - 1].as_u64(), secs[SECONDS - 2].as_u64(), secs[0].as_u64()), (Some(1), Some(2), Some(1)), "5,000 is 59 seconds ago, still inside");
        assert_eq!(s["last_minute"]["end_unix"].as_u64(), Some(5_060));
        let later = stats.snapshot_at(5_070);
        let total: u64 = later["last_minute"]["requests"].as_array().unwrap().iter().filter_map(Value::as_u64).sum();
        assert_eq!(total, 3, "the second that fell out of the window is gone");
    }
}
