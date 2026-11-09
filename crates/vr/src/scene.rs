use anyhow::{Context, Result, ensure};
use glam::{Mat4, Quat, Vec3, Vec4};
use openxr as xr;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

// A physical, seated globe. All geo math below uses unit-sphere coordinates.
// At Greenwich, east points right and north points up.
pub const RADIUS_M: f32 = 0.40;
pub fn surface(lat: f32, lon: f32) -> Vec3 {
    let (lat, lon) = (lat.to_radians(), lon.to_radians());
    Vec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
}

pub struct Marker {
    pub title: String,
    pub summary: String,
    pub lat: f32,
    pub lon: f32,
}

impl Marker {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self {
                title: "Mount Rainier".into(),
                summary: "Reference marker at the summit. The surrounding contour tile comes from SRTM elevation data.".into(),
                lat: 46.8523,
                lon: -121.7603,
            });
        };
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.query_row(
            "SELECT title,lat,lon,summary FROM stored_events WHERE lat BETWEEN -90 AND 90
            AND lon BETWEEN -180 AND 180 ORDER BY occurred_unix DESC, factal_id LIMIT 1",
            [],
            |row| {
                Ok(Self {
                    title: row.get(0)?,
                    summary: row.get(3)?,
                    lat: row.get(1)?,
                    lon: row.get(2)?,
                })
            },
        )
        .context("reading the most recent geolocated event from the desktop cache")
    }
}

pub fn pose(p: xr::Posef) -> Mat4 {
    let q = p.orientation;
    Mat4::from_rotation_translation(
        Quat::from_xyzw(q.x, q.y, q.z, q.w),
        Vec3::new(p.position.x, p.position.y, p.position.z),
    )
}

pub fn projection(fov: xr::Fovf) -> Mat4 {
    let (left, right) = (fov.angle_left.tan(), fov.angle_right.tan());
    let (down, up) = (fov.angle_down.tan(), fov.angle_up.tan());
    // Right-handed OpenXR (-Z forward), WebGPU depth [0,1], infinite far plane.
    Mat4::from_cols(
        Vec4::X * (2. / (right - left)),
        Vec4::Y * (2. / (up - down)),
        Vec4::new(
            (right + left) / (right - left),
            (up + down) / (up - down),
            -1.,
            -1.,
        ),
        Vec4::new(0., 0., -0.02, 0.),
    )
}

pub struct Globe {
    center: Vec3,
    heading: Quat,
    pub lon: f32,
    pub lat: f32,
    pub scale: f32,
}

impl Globe {
    pub fn place(views: &[xr::View], marker: &Marker) -> Self {
        let left = pose(views[0].pose);
        let right = pose(views[1].pose);
        let head = (left.w_axis.truncate() + right.w_axis.truncate()) * 0.5;
        let front = -(left.z_axis.truncate() + right.z_axis.truncate());
        let heading = Quat::from_rotation_y((-front.x).atan2(-front.z));
        Self {
            center: head + heading * Vec3::new(0., -0.18, -0.95),
            heading,
            scale: 1.,
            lon: marker.lon,
            lat: marker.lat,
        }
    }
    pub fn transform(&self) -> Mat4 {
        let rotation = self.heading
            * Quat::from_rotation_x(self.lat.to_radians())
            * Quat::from_rotation_y(-self.lon.to_radians());
        let r = RADIUS_M * self.scale;
        let center = self.center + self.heading * Vec3::Z * (RADIUS_M - r);
        Mat4::from_scale_rotation_translation(Vec3::splat(r), rotation, center)
    }
    pub fn zoom(&mut self, y: f32, dt: f32) {
        if y.abs() > 0.15 {
            self.scale = (self.scale * (y * dt * 1.5).exp()).clamp(1., 32.);
        }
    }
    pub fn rotate(&mut self, x: f32, y: f32, dt: f32) {
        let dead = |v: f32| if v.abs() > 0.15 { v } else { 0. };
        self.lon = (self.lon + dead(x) * dt * 65. / self.scale).rem_euclid(360.);
        self.lat = (self.lat + dead(y) * dt * 50. / self.scale).clamp(-89.9, 89.9);
    }
    pub fn brief_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::new(0.80, 0.32, 1.),
            self.heading,
            self.center + self.heading * Vec3::new(0., 0.59, 0.),
        )
    }
    pub fn aim_point(&self, aim: xr::Posef) -> Option<Vec3> {
        let local = self.transform().inverse() * pose(aim);
        let origin = local.w_axis.truncate();
        let dir = -local.z_axis.truncate().normalize();
        let b = origin.dot(dir);
        let disc = b * b - origin.length_squared() + 1.;
        if disc < 0. {
            return None;
        }
        let t = -b - disc.sqrt();
        (t > 0.).then_some(origin + dir * t)
    }
}

pub const MAX_VERTICES: usize = 262_144;
pub enum TileSource {
    Bundled,
    Native {
        path: std::path::PathBuf,
        key: [i32; 3],
    },
}

impl TileSource {
    pub fn read(self) -> Result<Vec<[f32; 3]>> {
        let mut vertices = Vec::new();
        let append = |vertices: &mut Vec<[f32; 3]>, lines: Vec<Vec<[f32; 3]>>| {
            for line in lines {
                for pair in line.windows(2) {
                    if vertices.len() >= MAX_VERTICES {
                        return;
                    }
                    vertices.extend_from_slice(pair);
                }
            }
        };
        let project = |lon, lat| surface(lat, lon).to_array();
        match self {
            Self::Bundled => {
                let bytes = include_bytes!("../assets/rainier.ctf");
                for (_, packed) in
                    tile_archive::contours::decode(bytes).map_err(anyhow::Error::msg)?
                {
                    append(
                        &mut vertices,
                        tile_archive::contours::decode_lines(packed, project)
                            .map_err(anyhow::Error::msg)?,
                    );
                }
            }
            Self::Native { path, key } => {
                let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let mut stmt = db.prepare(
                    "SELECT geom FROM contour_tiles WHERE zoom_bucket=?1
                    AND lat_bucket=?2 AND lon_bucket=?3 ORDER BY fid",
                )?;
                let mut rows = stmt.query(key)?;
                while let Some(row) = rows.next()? {
                    let bytes = row.get_ref(0)?.as_blob()?;
                    append(
                        &mut vertices,
                        tile_archive::gpkg::try_parse_gpkg_lines(bytes, project)
                            .context("corrupt contour geometry")?,
                    );
                    if vertices.len() == MAX_VERTICES {
                        break;
                    }
                }
            }
        }
        ensure!(!vertices.is_empty(), "selected tile has no contour lines");
        Ok(vertices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asymmetric_fov_maps_all_four_edges() {
        let fov = xr::Fovf {
            angle_left: -0.8,
            angle_right: 0.6,
            angle_down: -0.7,
            angle_up: 0.95,
        };
        let m = projection(fov);
        for (angle, axis, expected) in [
            (fov.angle_left, 0, -1.),
            (fov.angle_right, 0, 1.),
            (fov.angle_down, 1, -1.),
            (fov.angle_up, 1, 1.),
        ] {
            let mut p = Vec3::new(0., 0., -1.);
            p[axis] = angle.tan();
            assert!((m.project_point3(p)[axis] - expected).abs() < 1e-5);
        }
    }
    #[test]
    fn bundled_tile_and_native_reader_return_same_sphere_geometry() {
        let tile = TileSource::Bundled.read().unwrap();
        assert!(tile.len() > 4096 && tile.len() < MAX_VERTICES && tile.len().is_multiple_of(2));
        let db = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE contour_tiles(zoom_bucket,lat_bucket,lon_bucket,fid,geom)",
        )
        .unwrap();
        let line = tile_archive::contour_clip::encode(&[vec![(-121.8, 46.8), (-121.7, 46.9)]]);
        conn.execute("INSERT INTO contour_tiles VALUES(1,2,3,1,?1)", [line])
            .unwrap();
        let native = TileSource::Native {
            path: db.path().into(),
            key: [1, 2, 3],
        }
        .read()
        .unwrap();
        assert_eq!(native.len(), 2);
        for (got, want) in native
            .iter()
            .zip([surface(46.8, -121.8), surface(46.9, -121.7)])
        {
            assert!(Vec3::from_array(*got).distance(want) < 1e-6);
        }
    }
}
