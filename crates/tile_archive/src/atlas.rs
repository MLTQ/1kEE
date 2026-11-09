//! Persistent measured-height tiles shared by the offline builder and Atlas.
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};
pub const FILE_NAME: &str = "atlas_heights_v1.sqlite";
const APP_ID: i32 = 0x314b4154;
const VERSION: i32 = 1;
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Tier {
    Overview = 0,
    Terrain = 1,
    Land = 2,
}
impl Tier {
    pub const ALL: [Self; 3] = [Self::Overview, Self::Terrain, Self::Land];
    pub fn name(self) -> &'static str {
        match self {
            Self::Overview => "Overview · 5 arcmin",
            Self::Terrain => "Terrain · 15 arcsec",
            Self::Land => "Land detail · 1 arcsec",
        }
    }
    pub fn spacing(self) -> f64 {
        match self {
            Self::Overview => 300.,
            Self::Terrain => 15.,
            Self::Land => 1.,
        }
    }
    pub fn span(self) -> f64 {
        match self {
            Self::Overview => 10.,
            Self::Terrain => 1.,
            Self::Land => 0.25,
        }
    }
    pub fn core(self) -> usize {
        (self.span() * 3600. / self.spacing()).round() as usize
    }
    pub fn side(self) -> usize {
        self.core() + 2
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Key {
    pub tier: Tier,
    pub x: i32,
    pub y: i32,
}
impl Key {
    pub fn valid(self) -> bool {
        self.x >= 0
            && self.y >= 0
            && self.x < (360. / self.tier.span()) as i32
            && self.y < (180. / self.tier.span()) as i32
    }
    pub fn at(tier: Tier, lon: f64, lat: f64) -> Self {
        Self {
            tier,
            x: (((lon + 180.).rem_euclid(360.)) / tier.span()).floor() as i32,
            y: ((lat.clamp(-90., 90. - 1e-9) + 90.) / tier.span()).floor() as i32,
        }
    }
    /// West, south, east, north; the payload adds one sample of halo on each edge.
    pub fn bounds(self) -> [f64; 4] {
        let s = self.tier.span();
        let w = self.x as f64 * s - 180.;
        let b = self.y as f64 * s - 90.;
        [w, b, w + s, b + s]
    }
    pub fn source_bounds(self) -> [f64; 4] {
        let [w, s, e, n] = self.bounds();
        let d = self.tier.spacing() / 3600.;
        [w - d, s - d, e + d, n + d]
    }
}
pub fn validate_bounds(b: [f64; 4]) -> Result<()> {
    if !b.iter().all(|v| v.is_finite())
        || b[0] < -180.
        || b[0] >= 180.
        || b[2] <= b[0]
        || b[2] - b[0] > 360.
        || b[1] < -90.
        || b[3] > 90.
        || b[1] >= b[3]
    {
        return Err("Bounds must be west < east, south < north; longitude span ≤ 360°, latitude within ±90°. Use east > 180° to cross the date line.".into());
    }
    Ok(())
}
pub fn plan(bounds: [f64; 4], tiers: &[Tier]) -> Result<Vec<Key>> {
    validate_bounds(bounds)?;
    let mut keys = Vec::new();
    for &tier in &Tier::ALL {
        if !tiers.contains(&tier) {
            continue;
        }
        let span = tier.span();
        let nx = (360. / span) as i32;
        for y in ((bounds[1] + 90.) / span).floor() as i32..((bounds[3] + 90.) / span).ceil() as i32
        {
            let mut seen = std::collections::HashSet::new();
            for x in ((bounds[0] + 180.) / span).floor() as i32
                ..((bounds[2] + 180.) / span).ceil() as i32
            {
                let x = x.rem_euclid(nx);
                if seen.insert(x) {
                    keys.push(Key { tier, x, y });
                }
            }
        }
    }
    if keys.is_empty() {
        return Err("Select at least one Atlas detail tier".into());
    }
    Ok(keys)
}
fn validate_db(c: &Connection) -> Result<()> {
    let app: i32 = c
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let version: i32 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if app != APP_ID || version != VERSION {
        return Err("Unsupported Atlas height cache; choose a separate output file".into());
    }
    Ok(())
}
fn decode(bytes: &[u8], crc: i64, key: Key) -> Result<Vec<f32>> {
    if !key.valid()
        || bytes.len() != key.tier.side().pow(2) * 4
        || i64::from(crc32fast::hash(bytes)) != crc
    {
        return Err(format!(
            "Invalid Atlas tile {key:?}: size/checksum mismatch"
        ));
    }
    let values: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if values
        .iter()
        .any(|v| !v.is_nan() && (!v.is_finite() || !(-12000.0..=10000.0).contains(v)))
    {
        return Err(format!("Invalid Atlas heights in {key:?}"));
    }
    Ok(values)
}
fn get(c: &Connection, key: Key, source: Option<&str>) -> Result<Option<Vec<f32>>> {
    let mut stmt = c
        .prepare_cached("SELECT payload,crc,source FROM heights WHERE tier=?1 AND x=?2 AND y=?3")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![key.tier as i32, key.x, key.y])
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let saved: String = row.get(2).map_err(|e| e.to_string())?;
    if source.is_some_and(|s| s != saved) {
        return Ok(None);
    }
    let bytes = row
        .get_ref(0)
        .map_err(|e| e.to_string())?
        .as_blob()
        .map_err(|e| e.to_string())?;
    decode(bytes, row.get(1).map_err(|e| e.to_string())?, key).map(Some)
}
pub struct Writer {
    connection: Connection,
    _lock: File,
}
impl Writer {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("atlas.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("Atlas cache is already being built: {e}"))?;
        let exists = path.exists() && path.metadata().map_err(|e| e.to_string())?.len() > 0;
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        if exists {
            validate_db(&connection)?;
        } else {
            connection.execute_batch(&format!("BEGIN IMMEDIATE; PRAGMA application_id={APP_ID}; PRAGMA user_version={VERSION}; CREATE TABLE heights(tier INTEGER NOT NULL,x INTEGER NOT NULL,y INTEGER NOT NULL,source TEXT NOT NULL,payload BLOB NOT NULL,crc INTEGER NOT NULL,PRIMARY KEY(tier,x,y)); COMMIT;")).map_err(|e|e.to_string())?;
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-8192;",
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            connection,
            _lock: lock,
        })
    }
    pub fn reusable(&self, key: Key, source: &str) -> Result<bool> {
        get(&self.connection, key, Some(source)).map(|v| v.is_some())
    }
    pub fn put(&self, key: Key, source: &str, values: &[f32]) -> Result<()> {
        let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let crc = i64::from(crc32fast::hash(&bytes));
        decode(&bytes, crc, key)?;
        self.connection
            .execute(
                "INSERT OR REPLACE INTO heights VALUES (?1,?2,?3,?4,?5,?6)",
                params![key.tier as i32, key.x, key.y, source, bytes, crc],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn finish(&self) -> Result<()> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| e.to_string())
    }
}
pub struct Reader {
    connection: Connection,
    tiles: HashMap<Key, (Vec<f32>, u64)>,
    tick: u64,
}
impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(|e| e.to_string())?;
        validate_db(&connection)?;
        connection
            .pragma_update(None, "cache_size", -4096)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            connection,
            tiles: HashMap::new(),
            tick: 0,
        })
    }
    pub fn has_tier(&self, tier: Tier) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT 1 FROM heights WHERE tier=?1 LIMIT 1",
                [tier as i32],
                |_| Ok(()),
            )
            .optional()
            .map(|v| v.is_some())
            .map_err(|e| e.to_string())
    }
    pub fn sample(
        &mut self,
        bounds: [f64; 4],
        size: [usize; 2],
        tier: Tier,
    ) -> Result<Option<Vec<f32>>> {
        validate_bounds(bounds)?;
        if size[0] == 0
            || size[1] == 0
            || size[0] > 8192
            || size[1] > 8192
            || size[0].saturating_mul(size[1]) > 32_000_000
        {
            return Err("Atlas raster exceeds the image budget".into());
        }
        let [w, s, e, n] = bounds;
        let dx = (e - w) / size[0] as f64;
        let dy = (n - s) / size[1] as f64;
        let longitudes: Vec<_> = (0..size[0])
            .map(|col| (w + (col as f64 + 0.5) * dx + 180.).rem_euclid(360.) - 180.)
            .collect();
        let latitudes: Vec<_> = (0..size[1])
            .map(|row| n - (row as f64 + 0.5) * dy)
            .collect();
        let columns = tile_ranges(&longitudes, |lon| Key::at(tier, lon, 0.).x);
        let rows = tile_ranges(&latitudes, |lat| Key::at(tier, 0., lat).y);
        let mut out = vec![f32::NAN; size[0] * size[1]];
        // Traverse tiles, not scanlines: wide views must not repeatedly read
        // and decode every tile when a row spans more than the LRU capacity.
        for (yrange, ykey) in rows {
            for (xrange, xkey) in &columns {
                let key = Key {
                    tier,
                    x: *xkey,
                    y: ykey,
                };
                if !self.tiles.contains_key(&key) {
                    let Some(values) = get(&self.connection, key, None)? else {
                        return Ok(None);
                    };
                    if self.tiles.len() >= 64 {
                        let old = *self.tiles.iter().min_by_key(|(_, v)| v.1).unwrap().0;
                        self.tiles.remove(&old);
                    }
                    self.tiles.insert(key, (values, 0));
                }
                self.tick += 1;
                let (values, used) = self.tiles.get_mut(&key).unwrap();
                *used = self.tick;
                let [tw, _, _, tn] = key.bounds();
                let d = tier.spacing() / 3600.;
                for row in yrange.clone() {
                    let y = (tn - latitudes[row]) / d + 0.5;
                    for col in xrange.clone() {
                        let x = (longitudes[col] - tw) / d + 0.5;
                        out[row * size[0] + col] = bilinear(values, tier.side(), x, y);
                    }
                }
            }
        }
        Ok(Some(out))
    }
}
fn tile_ranges(values: &[f64], key: impl Fn(f64) -> i32) -> Vec<(std::ops::Range<usize>, i32)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < values.len() {
        let current = key(values[start]);
        let mut end = start + 1;
        while end < values.len() && key(values[end]) == current {
            end += 1;
        }
        out.push((start..end, current));
        start = end;
    }
    out
}
fn bilinear(values: &[f32], side: usize, x: f64, y: f64) -> f32 {
    let x = x.clamp(0., (side - 1) as f64);
    let y = y.clamp(0., (side - 1) as f64);
    let ix = (x.floor() as usize).min(side - 2);
    let iy = (y.floor() as usize).min(side - 2);
    let fx = (x - ix as f64) as f32;
    let fy = (y - iy as f64) as f32;
    let mut sum = 0.;
    let mut weight = 0.;
    for (index, w) in [
        (iy * side + ix, (1. - fx) * (1. - fy)),
        (iy * side + ix + 1, fx * (1. - fy)),
        ((iy + 1) * side + ix, (1. - fx) * fy),
        ((iy + 1) * side + ix + 1, fx * fy),
    ] {
        let value = values[index];
        if value.is_finite() {
            sum += value * w;
            weight += w;
        }
    }
    if weight > 0. { sum / weight } else { f32::NAN }
}
#[cfg(test)]
#[path = "atlas_tests.rs"]
mod tests;
