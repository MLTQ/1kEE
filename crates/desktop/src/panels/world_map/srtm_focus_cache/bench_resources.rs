//! Test-only process-tree RSS sampling; does not inspect unrelated app data.
use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Default)]
pub struct Peak {
    pub rss_kib: u64,
    pub gdal_processes: usize,
}

fn sample(output: &str, root: u32) -> Peak {
    let rows: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.parse::<u64>().ok()?,
                fields.next()?,
            ))
        })
        .collect();
    let mut pids = HashSet::from([root]);
    loop {
        let before = pids.len();
        for (pid, parent, _, _) in &rows {
            if pids.contains(parent) {
                pids.insert(*pid);
            }
        }
        if pids.len() == before {
            break;
        }
    }
    let mut peak = Peak::default();
    for (pid, _, rss, command) in rows {
        if pids.contains(&pid) && command.rsplit('/').next() != Some("ps") {
            peak.rss_kib += rss;
            peak.gdal_processes += usize::from(command.contains("gdal_"));
        }
    }
    peak
}

pub struct Sampler {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Peak>>,
}

impl Sampler {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut peak = Peak::default();
            while !cancelled.load(Ordering::Relaxed) {
                if let Ok(output) = std::process::Command::new("ps")
                    .args(["-axo", "pid=,ppid=,rss=,comm="])
                    .output()
                    && output.status.success()
                {
                    let current =
                        sample(&String::from_utf8_lossy(&output.stdout), std::process::id());
                    peak.rss_kib = peak.rss_kib.max(current.rss_kib);
                    peak.gdal_processes = peak.gdal_processes.max(current.gdal_processes);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            peak
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }

    pub fn finish(mut self) -> Peak {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn sample_includes_only_descendants_and_excludes_its_own_ps() {
    let p = sample(
        "4 2 200 /bin/ps\n1 0 50 /test\n2 1 300 /opt/gdal_contour\n3 0 999 /private-app\n",
        1,
    );
    assert_eq!(p.rss_kib, 350);
    assert_eq!(p.gdal_processes, 1);
}
