//! LINZ's merged national DEM follows the official 24×36 km Topo50 grid.
use super::*;
use std::collections::HashMap;

const COLLECTION: &str = "https://nz-elevation.s3-ap-southeast-2.amazonaws.com/new-zealand/new-zealand/dem_1m/2193/collection.json";
const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

fn grid_cells(bounds: Bounds) -> Result<Vec<String>> {
    // AS21's published COG origin: E1492000, N6234000, 24000×36000 at 1 m.
    // Increasing columns go east, row letter pairs go south (I and O omitted).
    let x0 = ((bounds.min_lon - 1_492_000.0) / 24_000.0).floor() as i32 + 21;
    let x1 = ((bounds.max_lon - 1_492_000.0) / 24_000.0).floor() as i32 + 21;
    let y0 = ((6_234_000.0 - bounds.max_lat) / 36_000.0).floor() as i32 + 16;
    let y1 = ((6_234_000.0 - bounds.min_lat) / 36_000.0).floor() as i32 + 16;
    if (x1 - x0 + 1) * (y1 - y0 + 1) > 16
        || x0 < 0
        || x1 > 99
        || y0 < 0
        || y1 >= (LETTERS.len() * LETTERS.len()) as i32
    {
        return Err(Error::Failed(
            "LINZ tile selection exceeds its bounds".into(),
        ));
    }
    let mut cells = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            cells.push(format!(
                "{}{}{x:02}",
                LETTERS[y as usize / LETTERS.len()] as char,
                LETTERS[y as usize % LETTERS.len()] as char
            ));
        }
    }
    Ok(cells)
}

pub(super) fn fetch(request: Request, scratch: &Scratch) -> Result<PathBuf> {
    let bounds = raster::transform_bounds(request.bounds, 2193)?;
    let cells = grid_cells(bounds)?;
    let collection = http::json(COLLECTION)?;
    let links = collection["links"]
        .as_array()
        .ok_or_else(|| Error::Failed("invalid LINZ catalog".into()))?;
    let items: HashMap<_, _> = links
        .iter()
        .filter(|link| link["rel"] == "item")
        .filter_map(|link| link["href"].as_str())
        .filter_map(|href| Some((href.rsplit('/').next()?.strip_suffix(".json")?, href)))
        .collect();
    let mut inputs = Vec::new();
    for cell in cells {
        let Some(href) = items.get(cell.as_str()) else {
            continue;
        };
        let url = http::resolve(COLLECTION, href)?;
        let item = http::json(&url)?;
        let asset = item["assets"]["visual"]["href"]
            .as_str()
            .ok_or_else(|| Error::Failed("missing LINZ elevation asset".into()))?;
        let href = http::resolve(&url, asset)?;
        if !href.ends_with(".tiff") && !href.ends_with(".tif") {
            return Err(Error::Failed("unexpected LINZ elevation format".into()));
        }
        inputs.push(format!("/vsicurl/{href}"));
    }
    raster::warp(&inputs, request, scratch, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_national_grid_cells_and_crossings() {
        assert_eq!(
            grid_cells(Bounds {
                min_lon: 1_492_001.0,
                max_lon: 1_492_002.0,
                min_lat: 6_233_990.0,
                max_lat: 6_233_999.0
            })
            .unwrap(),
            ["AS21"]
        );
        // BQ is 22 rows below AS; I and O are omitted.
        let row = LETTERS.iter().position(|&c| c == b'Q').unwrap() + LETTERS.len();
        let north = 6_234_000.0 - (row as f64 - 16.0) * 36_000.0;
        assert_eq!(
            grid_cells(Bounds {
                min_lon: 1_732_001.0,
                max_lon: 1_732_002.0,
                min_lat: north - 10.0,
                max_lat: north - 1.0
            })
            .unwrap(),
            ["BQ31"]
        );
        assert_eq!(
            grid_cells(Bounds {
                min_lon: 1_515_999.0,
                max_lon: 1_516_001.0,
                min_lat: 6_233_990.0,
                max_lat: 6_233_999.0
            })
            .unwrap(),
            ["AS21", "AS22"]
        );
    }
}
