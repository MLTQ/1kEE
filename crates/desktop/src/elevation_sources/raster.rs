use super::*;
use crate::panels::world_map::srtm_focus_cache::gdal;
use std::io::Write;
use std::process::{Command, Stdio};

fn run(mut command: Command) -> Result<()> {
    command.stdout(Stdio::null());
    gdal::run_command_with_timeout(command, "international elevation", remaining()?)?;
    Ok(())
}

pub(super) fn warp(
    inputs: &[String],
    request: Request,
    scratch: &Scratch,
    libertiff: bool,
) -> Result<PathBuf> {
    if inputs.is_empty() {
        return Err(Error::NoCoverage);
    }
    let output = scratch.path("normalized.tif");
    let b = request.bounds;
    let mut command = Command::new(gdal::gdal_tool_path("gdalwarp"));
    command
        .args([
            "-q",
            "-overwrite",
            "-of",
            "GTiff",
            "-ot",
            "Float32",
            "-t_srs",
            "EPSG:4326",
            "-te_srs",
            "EPSG:4326",
            "-te",
        ])
        .args([
            b.min_lon.to_string(),
            b.min_lat.to_string(),
            b.max_lon.to_string(),
            b.max_lat.to_string(),
        ])
        .args([
            "-ts",
            &request.width.to_string(),
            &request.height.to_string(),
            "-r",
            "bilinear",
            "-et",
            "0",
            "-dstnodata",
            "-999999",
            "-wm",
            "32",
            "-co",
            "COMPRESS=DEFLATE",
            "-co",
            "PREDICTOR=3",
            "-wo",
            "NUM_THREADS=1",
            "--config",
            "GDAL_CACHEMAX",
            "32",
            "--config",
            "GDAL_DISABLE_READDIR_ON_OPEN",
            "EMPTY_DIR",
            "--config",
            "GDAL_HTTP_CONNECTTIMEOUT",
            "15",
            "--config",
            "GDAL_HTTP_TIMEOUT",
            "30",
            "--config",
            "GDAL_HTTP_MAX_RETRY",
            "1",
        ]);
    // LINZ uses LERC-compressed COGs. macOS's ordinary GTiff driver can lack
    // that codec; GDAL 3.11+'s built-in LIBERTIFF reader handles it directly.
    if libertiff {
        command.args(["-if", "LIBERTIFF"]);
    }
    command.args(inputs).arg(&output);
    run(command)?;
    validate(&output, request, scratch)?;
    Ok(output)
}

fn validate(path: &Path, request: Request, scratch: &Scratch) -> Result<()> {
    let band = scratch.path("check.bil");
    let mut command = Command::new(gdal::gdal_tool_path("gdal_translate"));
    command
        .args(["-q", "-of", "EHdr", "-ot", "Float32"])
        .arg(path)
        .arg(&band);
    run(command)?;
    let bytes = std::fs::read(band)?;
    if bytes.len() != request.width as usize * request.height as usize * 4 {
        return Err(Error::Failed(
            "unexpected elevation raster dimensions".into(),
        ));
    }
    let valid = bytes.chunks_exact(4).any(|p| {
        let height = f32::from_le_bytes(p.try_into().unwrap());
        height.is_finite() && (-12_000.0..=10_000.0).contains(&height)
    });
    if valid {
        Ok(())
    } else {
        Err(Error::NoCoverage)
    }
}

pub(super) fn float_grid(samples: &[f32], request: Request, scratch: &Scratch) -> Result<PathBuf> {
    if samples.len() != request.width as usize * request.height as usize {
        return Err(Error::Failed("unexpected elevation sample count".into()));
    }
    let mut band = std::io::BufWriter::new(std::fs::File::create(scratch.path("samples.bin"))?);
    for height in samples {
        band.write_all(&height.to_le_bytes())?;
    }
    band.flush()?;
    let b = request.bounds;
    let w = request.width;
    let h = request.height;
    let dx = (b.max_lon - b.min_lon) / f64::from(w);
    let dy = -(b.max_lat - b.min_lat) / f64::from(h);
    let xml = format!(
        r#"<VRTDataset rasterXSize="{w}" rasterYSize="{h}">
<SRS>EPSG:4326</SRS><GeoTransform>{}, {dx}, 0, {}, 0, {dy}</GeoTransform>
<VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand">
<NoDataValue>{NODATA}</NoDataValue><SourceFilename relativeToVRT="1">samples.bin</SourceFilename>
<ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>{}</LineOffset><ByteOrder>LSB</ByteOrder>
</VRTRasterBand></VRTDataset>"#,
        b.min_lon,
        b.max_lat,
        w * 4
    );
    let path = scratch.path("samples.vrt");
    std::fs::write(&path, xml)?;
    Ok(path)
}

/// A small local-only coordinate transform (no remote rasters or grid fetches).
pub(super) fn transform_bounds(bounds: Bounds, epsg: u32) -> Result<Bounds> {
    let mut child = Command::new(gdal::gdal_tool_path("gdaltransform"))
        .args([
            "-s_srs",
            "EPSG:4326",
            "-t_srs",
            &format!("EPSG:{epsg}"),
            "-output_xy",
        ])
        .env("PROJ_NETWORK", "OFF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let wait = (|| -> Result<()> {
        let mut input = child.stdin.take().unwrap();
        for x in [
            bounds.min_lon,
            (bounds.min_lon + bounds.max_lon) * 0.5,
            bounds.max_lon,
        ] {
            for y in [
                bounds.min_lat,
                (bounds.min_lat + bounds.max_lat) * 0.5,
                bounds.max_lat,
            ] {
                writeln!(input, "{x} {y}")?;
            }
        }
        drop(input);
        // Nine output lines fit in the stdout pipe, so wait without a reader
        // thread while retaining the same cancellation/deadline as downloads.
        loop {
            remaining()?;
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })();
    if let Err(error) = wait {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(Error::Failed(
            "elevation coordinate transform failed".into(),
        ));
    }
    let mut result = Bounds {
        min_lon: f64::INFINITY,
        min_lat: f64::INFINITY,
        max_lon: f64::NEG_INFINITY,
        max_lat: f64::NEG_INFINITY,
    };
    let mut count = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let coordinates: Vec<f64> = line
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if coordinates.len() != 2 || !coordinates.iter().all(|v| v.is_finite()) {
            return Err(Error::Failed("invalid projected elevation bounds".into()));
        }
        result.min_lon = result.min_lon.min(coordinates[0]);
        result.max_lon = result.max_lon.max(coordinates[0]);
        result.min_lat = result.min_lat.min(coordinates[1]);
        result.max_lat = result.max_lat.max(coordinates[1]);
        count += 1;
    }
    if count != 9 {
        return Err(Error::Failed(
            "incomplete elevation coordinate transform".into(),
        ));
    }
    Ok(result)
}
