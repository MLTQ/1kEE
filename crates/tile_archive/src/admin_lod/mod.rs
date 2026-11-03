//! Persistent boundary generalizations and spatially addressable original detail.
mod build;
mod geometry;
mod source;
use cell_format::CellPoint;
pub use geometry::{ERRORS, FULL, bounds, importance, simplify, tier_for_scale, unwrap};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A padded geographic envelope in fixed microdegrees. Snapping outwards lets
/// neighboring camera frames reuse one immutable query result, including at ±180°.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window([i32; 4]);
impl Window {
    pub const WORLD: Self = Self([-180000000, -90000000, 180000000, 90000000]);
    pub fn around(b: [f32; 4]) -> Self {
        if !b.iter().all(|x| x.is_finite()) {
            return Self::WORLD;
        }
        let cell = 2f32.powf((b[2] - b[0]).max(b[3] - b[1]).max(0.02).log2().floor() - 1.);
        let lo = |v: f32| ((v / cell).floor() - 0.5) * cell;
        let hi = |v: f32| ((v / cell).ceil() + 0.5) * cell;
        let (mut w, mut e) = (lo(b[0]), hi(b[2]));
        if e - w >= 360. {
            (w, e) = (-180., 180.)
        } else {
            let shift = 360. * (((w + e) * 0.5) / 360.).round();
            w -= shift;
            e -= shift;
        }
        Self(
            [w, lo(b[1]).clamp(-90., 90.), e, hi(b[3]).clamp(-90., 90.)]
                .map(|v| (v * 1e6).round() as i32),
        )
    }
    fn degrees(self) -> [f32; 4] {
        self.0.map(|n| n as f32 / 1e6)
    }
    pub fn intersects(self, b: [f32; 4]) -> bool {
        let [w, s, e, n] = self.degrees();
        let shift = 360. * (((w + e - b[0] - b[2]) * 0.5) / 360.).round();
        b[1] <= n && b[3] >= s && b[0] + shift <= e && b[2] + shift >= w
    }
}

pub struct Piece {
    pub relation: i64,
    pub complete: bool,
    pub points: Vec<CellPoint>,
}
pub struct Archive {
    db: Connection,
    source: File,
    stamp: String,
    pub invalid_rings: u64,
}
fn stamp(metadata: &Metadata) -> Result<String> {
    let time = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        metadata.ino()
    };
    #[cfg(not(unix))]
    let inode = 0;
    Ok(format!("v1:{}:{time}:{inode}", metadata.len()))
}
pub fn path_for(source: &Path) -> PathBuf {
    source.with_extension("lod-v1.sqlite")
}
impl Archive {
    pub fn open(source: &Path, level: u8) -> Result<Self> {
        let mut file = File::open(source)?;
        let identity = stamp(&file.metadata()?)?;
        let path = path_for(source);
        let read = |path: &Path| -> Result<(Connection, u64)> {
            let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let (saved, invalid): (String, u64) =
                db.query_row("SELECT stamp,invalid_rings FROM source", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
            if saved != identity {
                return Err("Boundary LOD source changed".into());
            }
            Ok((db, invalid))
        };
        if let Ok((db, invalid_rings)) = read(&path) {
            return Ok(Self {
                db,
                source: file,
                stamp: identity,
                invalid_rings,
            });
        }
        let (path, tmp) = match tempfile::NamedTempFile::new_in(
            path.parent().ok_or("Missing cache directory")?,
        ) {
            Ok(tmp) => (path, tmp),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                use std::hash::{DefaultHasher, Hash, Hasher};
                let mut hash = DefaultHasher::new();
                source.canonicalize()?.hash(&mut hash);
                let dir = std::env::temp_dir().join("1kee-admin-lods-v1");
                fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{:016x}.sqlite", hash.finish()));
                if let Ok((db, invalid_rings)) = read(&path) {
                    return Ok(Self {
                        db,
                        source: file,
                        stamp: identity,
                        invalid_rings,
                    });
                }
                (path, tempfile::NamedTempFile::new_in(dir)?)
            }
            Err(err) => return Err(err.into()),
        };
        let db = Connection::open(tmp.path())?;
        build::populate(&db, &mut file, level, &identity)?;
        if stamp(&fs::metadata(source)?)? != identity {
            return Err("Boundary source replaced during LOD build".into());
        }
        db.close().map_err(|(_, e)| e)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path)?;
        let (db, invalid_rings) = read(&path)?;
        Ok(Self {
            db,
            source: file,
            stamp: identity,
            invalid_rings,
        })
    }
    pub fn segment_counts(&self) -> Result<[u64; ERRORS.len()]> {
        let mut q = self
            .db
            .prepare("SELECT tier,sum(n-1) FROM pieces GROUP BY tier")?;
        let mut counts = [0; ERRORS.len()];
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            *counts
                .get_mut(row.get::<_, usize>(0)?)
                .ok_or("Invalid detail tier in index")? = row.get(1)?;
        }
        Ok(counts)
    }
    /// Lines read intersecting chunks. Fills read whole intersecting rings so
    /// an enclosing polygon or hole with all vertices off screen is still correct.
    pub fn query(
        &mut self,
        tier: usize,
        window: Window,
        shade: bool,
        mut emit: impl FnMut(Piece),
    ) -> Result<()> {
        if stamp(&self.source.metadata()?)? != self.stamp {
            return Err("Boundary source changed since LOD preparation".into());
        }
        if tier > FULL {
            return Err("Invalid boundary detail".into());
        }
        let [w, s, e, n] = window.degrees();
        // For a world view, walk rings in file order. Spatial order causes random
        // reads across the entire sidecar when almost every piece is needed.
        let (tables, filter, order) = if window == Window::WORLD {
            (
                "rings r CROSS JOIN pieces p ON p.ring=r.id",
                "p.tier=?1",
                "ORDER BY r.id,p.seq",
            )
        } else if shade {
            (
                "ring_boxes b CROSS JOIN rings r ON r.id=b.id CROSS JOIN pieces p ON p.ring=r.id",
                "p.tier=?1",
                "ORDER BY p.ring,p.seq",
            )
        } else {
            (
                "piece_boxes b CROSS JOIN pieces p ON p.id=b.id CROSS JOIN rings r ON r.id=p.ring",
                "b.lod_min<=?1 AND b.lod_max>=?1",
                "",
            )
        };
        let geographic = if window == Window::WORLD {
            ""
        } else {
            "AND b.south<=?5 AND b.north>=?3 AND (
               (b.west<=?4 AND b.east>=?2) OR (b.west<=?4+360 AND b.east>=?2+360)
               OR (b.west<=?4-360 AND b.east>=?2-360))"
        };
        let sql = format!(
            "SELECT r.id, r.relation, bad.id IS NULL, p.n, p.offset, p.stride, p.anchor, p.data
            FROM {tables} LEFT JOIN bad ON bad.id=r.relation WHERE {filter} {geographic} {order}"
        );
        let mut stmt = self.db.prepare(&sql)?;
        let mut rows = if window == Window::WORLD {
            stmt.query([tier])?
        } else {
            stmt.query(params![tier, w, s, e, n])?
        };
        let mut pending: Option<(i64, Piece)> = None;
        while let Some(row) = rows.next()? {
            let ring: i64 = row.get(0)?;
            let count: usize = row.get(3)?;
            if !(2..=257).contains(&count) {
                return Err("Invalid LOD piece length".into());
            }
            let mut points = if tier == FULL {
                let stride: usize = row.get(5)?;
                if stride != 8 && stride != 12 {
                    return Err("Invalid boundary stride".into());
                }
                let mut bytes = vec![0; stride * count];
                self.source.seek(SeekFrom::Start(row.get(4)?))?;
                self.source.read_exact(&mut bytes)?;
                let mut pts = source::decode(&bytes, stride);
                unwrap(&mut pts, row.get(6)?);
                pts
            } else {
                let bytes = row.get_ref(7)?.as_blob()?;
                if bytes.len() != 8 * count {
                    return Err("Invalid LOD coordinates".into());
                }
                source::decode(bytes, 8)
            };
            if shade
                && let Some((last, part)) = &mut pending
                && *last == ring
            {
                part.points.extend(points.drain(1..));
                continue;
            }
            if let Some((_, part)) = pending.take() {
                emit(part)
            }
            let part = Piece {
                relation: row.get(1)?,
                complete: row.get(2)?,
                points,
            };
            if shade {
                pending = Some((ring, part));
            } else {
                emit(part);
            }
        }
        if let Some((_, part)) = pending {
            emit(part)
        }
        if stamp(&self.source.metadata()?)? != self.stamp {
            return Err("Boundary source changed while reading LOD".into());
        }
        Ok(())
    }
}
