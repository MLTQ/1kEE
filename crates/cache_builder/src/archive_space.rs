//! Destination-space reporting for the temporary archive, before cleanup.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const RESERVE_BYTES: u64 = 1024 * 1024 * 1024;

pub fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
}

fn parse_available(output: &str) -> Option<u64> {
    output.lines().skip(1).find_map(|line| {
        line.split_whitespace()
            .nth(3)?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}

fn available(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|p| p.exists())?.canonicalize().ok()?;
    let output = std::process::Command::new("df")
        .arg("-Pk")
        .arg(existing)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    parse_available(&String::from_utf8_lossy(&output.stdout))
}

fn require_headroom(free: Option<u64>) -> Result<(), String> {
    if let Some(free) = free.filter(|&n| n < RESERVE_BYTES) {
        return Err(format!(
            "Packing stopped before filling the output drive: {} free; keeping {} of working space. Free more space or choose another output drive.",
            gb(free),
            gb(RESERVE_BYTES)
        ));
    }
    Ok(())
}

pub struct SpaceMonitor {
    output: PathBuf,
    stage: PathBuf,
    last_check: Option<Instant>,
    last_report: Option<Instant>,
}

impl SpaceMonitor {
    pub fn new(output: PathBuf, stage: PathBuf) -> Self {
        Self {
            output,
            stage,
            last_check: None,
            last_report: None,
        }
    }

    fn destination(&self) -> &Path {
        self.output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }

    fn snapshot(&self, free: Option<u64>) -> String {
        let written = std::fs::metadata(&self.stage).map(|m| m.len()).unwrap_or(0);
        format!(
            "Archive output: {}. Temporary archive: {} written; output drive free: {}.",
            self.output.display(),
            gb(written),
            free.map(gb).unwrap_or_else(|| "unknown".into())
        )
    }

    /// Worker-only, best effort: check at most once per second, report every five.
    /// This does not reserve capacity against concurrent writers or a large tile.
    pub fn check(&mut self, progress: &mut dyn FnMut(String)) -> Result<(), String> {
        if self
            .last_check
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        let free = available(self.destination());
        self.last_check = Some(Instant::now());
        if self
            .last_report
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            progress(self.snapshot(free));
            self.last_report = Some(Instant::now());
        }
        require_headroom(free)
    }

    pub fn failure(&self, error: &str) -> String {
        // Capture this while the staging file still exists. Reporting only
        // after RAII cleanup hides the space consumed by the failed pack.
        let written = std::fs::metadata(&self.stage).map(|m| m.len()).unwrap_or(0);
        let free = available(self.destination());
        format!(
            "Packing failed after {} was written; {} free on the output drive.\n{error}\n{} No completed archive was published. The temporary output is cleaned up on failure, so its space becomes available again. Original source files are kept.",
            gb(written),
            free.map(gb).unwrap_or_else(|| "unknown space".into()),
            self.snapshot(free)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_available_space_and_does_not_treat_unknown_as_zero() {
        assert_eq!(
            parse_available(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk6 999 100 899 11% /Volumes/My Drive\n"
            ),
            Some(899 * 1024)
        );
        assert_eq!(parse_available("df: failed"), None);
        assert!(require_headroom(None).is_ok());
        assert!(require_headroom(Some(RESERVE_BYTES)).is_ok());
        assert!(require_headroom(Some(RESERVE_BYTES - 1)).is_err());
        assert!(require_headroom(Some(0)).is_err());
        assert_eq!(gb(23_830_000_000), "23.83 GB");
    }

    #[test]
    fn failure_captures_temporary_bytes_and_destination_before_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "1kee-space-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let output = root.join("world.1ka");
        let stage = root.join("owned.part");
        let file = std::fs::File::create(&stage).unwrap();
        file.set_len(10_000_000).unwrap();
        drop(file);
        let monitor = SpaceMonitor::new(output.clone(), stage.clone());
        let error = monitor.failure("database or disk is full");
        assert!(
            error.starts_with("Packing failed after 0.01 GB was written"),
            "{error}"
        );
        assert!(error.contains("database or disk is full"));
        assert!(error.contains(&output.display().to_string()));
        assert!(stage.exists()); // reporting does not delete the owner's output
        assert!(!output.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
