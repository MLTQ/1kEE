//! Shutdown-aware GDAL execution with bounded, fail-fast write diagnostics.
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn fatal_write_error(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    [
        "database or disk is full",
        "no space left on device",
        "cannot write linestring",
        "no such table: contour",
    ]
    .iter()
    .any(|pattern| line.contains(pattern))
}

pub(super) fn run(
    mut command: Command,
    label: &str,
    timeout: Duration,
    shutdown: &AtomicBool,
    active: &Mutex<HashSet<u32>>,
) -> io::Result<()> {
    if shutdown.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "GDAL cancelled during shutdown",
        ));
    }
    if super::super::timings::enabled() {
        command.stdout(Stdio::null());
    }
    command.stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child.id();
    if let Ok(mut guard) = active.lock() {
        guard.insert(pid);
    }
    let failed_write = Arc::new(AtomicBool::new(false));
    let flag = failed_write.clone();
    let stderr = child.stderr.take().expect("piped GDAL stderr");
    let diagnostic_label = label.to_owned();
    let diagnostics = std::thread::spawn(move || {
        let mut suppressed = 0;
        for (count, line) in io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .enumerate()
        {
            let first_fatal = fatal_write_error(&line) && !flag.swap(true, Ordering::Relaxed);
            if count < 8 || first_fatal {
                eprintln!("[1kEE] {diagnostic_label}: {line}");
            } else {
                suppressed += 1;
            }
        }
        if suppressed > 0 {
            eprintln!(
                "[1kEE] {diagnostic_label}: suppressed {suppressed} further diagnostic lines"
            );
        }
    });
    let start = Instant::now();
    let result = (|| loop {
        if failed_write.load(Ordering::Relaxed) {
            break Err(io::Error::other(format!(
                "{label} aborted after a storage/contour write failure"
            )));
        }
        if let Some(status) = child.try_wait()? {
            break if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "{label} failed with status {status}"
                )))
            };
        }
        if shutdown.load(Ordering::Relaxed) {
            break Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!("{label} cancelled during shutdown"),
            ));
        }
        if start.elapsed() >= timeout {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{label} timed out after {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Ok(mut guard) = active.lock() {
        guard.remove(&pid);
    }
    let _ = diagnostics.join();
    // An error line can arrive between the last flag read and process exit.
    if result.is_ok() && failed_write.load(Ordering::Relaxed) {
        return Err(io::Error::other(format!(
            "{label} reported a storage/contour write failure"
        )));
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn failed_writer_is_stopped_and_reaped_instead_of_spamming_errors() {
        let active = Mutex::new(HashSet::new());
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf '%s\\n' 'ERROR 1: cannot write linestring' >&2; exec sleep 30",
        ]);
        let start = Instant::now();
        assert!(
            run(
                command,
                "test GDAL failure",
                Duration::from_secs(10),
                &AtomicBool::new(false),
                &active
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(active.lock().unwrap().is_empty());
    }
    #[test]
    fn successful_exit_cannot_hide_a_fatal_write_diagnostic() {
        let active = Mutex::new(HashSet::new());
        let mut command = Command::new("sh");
        command.args(["-c", "for n in 1 2 3 4 5 6 7 8 9; do echo 'Warning: optional metadata' >&2; done; echo 'ERROR 1: no such table: contour' >&2; exit 0"]);
        assert!(
            run(
                command,
                "test schema failure",
                Duration::from_secs(5),
                &AtomicBool::new(false),
                &active
            )
            .is_err()
        );
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'Warning: optional metadata' >&2; exit 0"]);
        assert!(
            run(
                command,
                "test warning",
                Duration::from_secs(5),
                &AtomicBool::new(false),
                &active
            )
            .is_ok()
        );
    }
}
