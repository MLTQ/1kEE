//! Strict, bounded TSV parsing; public source identity is not an OSM identity.
use super::{Position, Snapshot};
use crate::model::GeoPoint;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ROWS: usize = 1_000_000;

pub(super) fn load(path: &Path) -> Result<Snapshot, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("camera TSV exceeds 128 MiB".into());
    }
    // Read one extra byte to detect growth/truncation at the byte ceiling.
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("camera TSV exceeds 128 MiB".into());
    }
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<Snapshot, String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_reader(bytes);
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim_start_matches('\u{feff}') == name)
            .ok_or_else(|| format!("camera TSV missing {name} column"))
    };
    let lat = column("lat")?;
    let lon = column("lon")?;
    let id = column("OBJECTID")?;
    let active = column("active")?;
    let status = column("status")?;
    let mut seen = HashSet::new();
    let mut snapshot = Snapshot::default();
    for record in reader.records() {
        let record = record.map_err(|e| format!("invalid camera TSV: {e}"))?;
        snapshot.rows += 1;
        if snapshot.rows > MAX_ROWS {
            return Err("camera TSV exceeds one million records".into());
        }
        let point = record[lat]
            .trim()
            .parse::<f64>()
            .ok()
            .zip(record[lon].trim().parse::<f64>().ok());
        let object_id = record[id].trim().parse::<u64>().ok().filter(|id| *id > 0);
        let (Some((lat, lon)), Some(id)) = (point, object_id) else {
            snapshot.skipped += 1;
            continue;
        };
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
            || (lat == 0.0 && lon == 0.0)
            || !seen.insert(id)
        {
            snapshot.skipped += 1;
            continue;
        }
        let in_service = record[active].trim() == "1" && record[status].trim() == "inService";
        snapshot.in_service += usize::from(in_service);
        snapshot.positions.push(Position {
            location: GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            },
            in_service,
        });
    }
    if snapshot.positions.is_empty() {
        return Err("camera TSV has no valid positions".into());
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_reordered_fields_statuses_and_invalid_points() {
        let input = b"OBJECTID\tname\tlon\tlat\tstatus\tactive\r\n\
1\t\"Road\twith a tab\"\t-74.0\t40.0\tinService\t1\r\n\
2\t\"two\nlines\"\t-74.1\t40.1\tinPlanning\t1\r\n\
3\tretired\t-74.2\t40.2\tinService\t0\r\n\
4\tmissing\t0\t0\tinService\t1\r\n\
5\tbad\t-74\tNaN\tinService\t1\r\n\
1\tduplicate\t-74\t40\tinService\t1\r\n";
        let snapshot = parse(input).unwrap();
        assert_eq!(
            (
                snapshot.rows,
                snapshot.positions.len(),
                snapshot.in_service,
                snapshot.skipped
            ),
            (6, 3, 1, 3)
        );
        assert_eq!(snapshot.positions[0].location.lon, -74.0);
    }

    #[test]
    fn malformed_and_empty_input_cannot_replace_a_snapshot() {
        for body in [
            "<html>error</html>",
            "lat\tlon\tOBJECTID\tactive\tstatus\n",
            "lat\tlon\tOBJECTID\tactive\tstatus\n40\t-74\t1\t1\tinService\n41\t-75\n",
        ] {
            assert!(parse(body.as_bytes()).is_err());
        }
    }

    #[test]
    #[ignore = "reads the operator-selected local Flock inventory; no network or writes"]
    fn downloaded_inventory_loads_through_the_production_parser() {
        let path = std::env::var("ONEKEE_FLOCK_TSV").expect("set ONEKEE_FLOCK_TSV");
        let started = std::time::Instant::now();
        let snapshot = load(Path::new(&path)).unwrap();
        eprintln!(
            "Flock: {} records, {} valid positions, {} in service, {} skipped; {:.2}s",
            snapshot.rows,
            snapshot.positions.len(),
            snapshot.in_service,
            snapshot.skipped,
            started.elapsed().as_secs_f64()
        );
        assert_eq!(snapshot.positions.len() + snapshot.skipped, snapshot.rows);
        assert!(snapshot.in_service > 0);
    }
}
