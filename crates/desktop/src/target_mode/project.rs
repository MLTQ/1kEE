//! Bounded XY-only GDAL transforms. Source point Z never enters the process.
use crate::panels::world_map::srtm_focus_cache::gdal;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn run(mut command: Command, active: &Path, label: &str) -> Result<(), String> {
    let mut child = command
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{label}: {e}"))?;
    let start = Instant::now();
    let result = (|| loop {
        if !active.is_file()
            || gdal::shutdown_requested().load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err("Target closed".to_string());
        }
        if start.elapsed() > Duration::from_secs(30) {
            return Err(format!("{label} timed out"));
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("{label} failed; check the configured GDAL tools"))
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub(crate) fn xy(crs: &str, points: &mut [[f64; 2]], active: &Path) -> Result<(), String> {
    if points.is_empty() {
        return Ok(());
    }
    if points.len() > 32_768 {
        return Err("Coordinate batch exceeds the Target limit".into());
    }
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = temp.path().join("xy.txt");
    let output = temp.path().join("geographic.txt");
    let mut writer = BufWriter::new(File::create(&input).map_err(|e| e.to_string())?);
    for [x, y] in points.iter() {
        writeln!(writer, "{x:.17} {y:.17}").map_err(|e| e.to_string())?;
    }
    writer.flush().map_err(|e| e.to_string())?;
    drop(writer);
    let mut command = Command::new(gdal::gdal_tool_path("gdaltransform"));
    command
        .args(["-s_srs", crs, "-t_srs", "EPSG:4326", "-output_xy"])
        .env("PROJ_NETWORK", "OFF")
        .stdin(File::open(&input).map_err(|e| e.to_string())?)
        .stdout(File::create(&output).map_err(|e| e.to_string())?);
    run(command, active, "Point horizontal transform")?;
    if output.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
        return Err("Oversized coordinate response".into());
    }
    let result = std::fs::read_to_string(output).map_err(|e| e.to_string())?;
    let mut count = 0;
    for (point, line) in points.iter_mut().zip(result.lines()) {
        let values = line
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if values.len() != 2 || !values.iter().all(|v| v.is_finite()) {
            return Err("Invalid transformed point".into());
        }
        *point = [values[0], values[1]];
        count += 1;
    }
    if count != points.len() || result.lines().count() != points.len() {
        return Err("Incomplete coordinate transform".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_xy_keeps_order_precision_and_cancellation() {
        let active = tempfile::NamedTempFile::new().unwrap();
        let mut p = [[180352.3369445393, 706671.4663639435], [181000., 700000.]];
        xy("EPSG:2039", &mut p, active.path()).unwrap();
        assert!((p[0][0] - 34.78855934677259).abs() < 0.00002);
        assert!(p[0][1] > p[1][1]);
        let mut same = p;
        xy("EPSG:4326", &mut same, active.path()).unwrap();
        assert!(
            p.iter()
                .zip(same)
                .all(|(a, b)| (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12)
        );
        let marker = active.path().to_owned();
        drop(active);
        assert!(xy("EPSG:2039", &mut p, &marker).is_err());
    }
}
