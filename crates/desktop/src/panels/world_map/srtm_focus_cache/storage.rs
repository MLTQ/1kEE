//! Best-effort storage admission for background terrain builders.
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HEADROOM_BYTES: u64 = 1024 * 1024 * 1024;
static LAST_WARNING: AtomicU64 = AtomicU64::new(0);

fn available_bytes(output: &str) -> Option<u64> {
    output.lines().skip(1).find_map(|line| {
        line.split_whitespace()
            .nth(3)?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}

/// Workers only: never execute a filesystem query from paint. Unknown space
/// does not block a build; ordinary I/O errors still abort and clean it up.
pub(super) fn require_room(path: &Path) -> io::Result<()> {
    let Some(existing) = path.ancestors().find(|p| p.exists()) else {
        return Ok(());
    };
    let free = std::process::Command::new("df")
        .arg("-Pk")
        .arg(existing)
        .output()
        .ok()
        .filter(|r| r.status.success())
        .and_then(|r| available_bytes(&String::from_utf8_lossy(&r.stdout)));
    if let Some(free) = free.filter(|n| *n < HEADROOM_BYTES) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let previous = LAST_WARNING.load(Ordering::Relaxed);
        if now.saturating_sub(previous) >= 30
            && LAST_WARNING
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            eprintln!(
                "[1kEE] Terrain builds paused: {} has only {} MiB free; at least 1024 MiB headroom is required. Free space or choose another cache location.",
                existing.display(),
                free / (1024 * 1024)
            );
        }
        return Err(io::Error::other(
            "insufficient free space for terrain generation",
        ));
    }
    Ok(())
}

#[test]
fn parses_posix_df_without_confusing_free_bytes_with_capacity_or_mount_spaces() {
    assert_eq!(
        available_bytes(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk5 100000 99876 124 100% /Volumes/My Drive\n"
        ),
        Some(124 * 1024)
    );
    assert_eq!(
        available_bytes("Filesystem\n/dev/disk5 1000 1000 0 100% /Data"),
        Some(0)
    );
    assert_eq!(available_bytes("df: unavailable"), None);
}
