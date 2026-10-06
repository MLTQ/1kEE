//! Single-flight background snapshots. The caller only polls a channel.
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub(super) struct Snapshot<K, V> {
    requested: Option<K>,
    current: Option<(K, Arc<V>)>,
    pending: Option<(u64, K, mpsc::Receiver<Option<Arc<V>>>)>,
    epoch: u64,
    retry: Option<Instant>,
}
impl<K, V> Default for Snapshot<K, V> {
    fn default() -> Self {
        Self {
            requested: None,
            current: None,
            pending: None,
            epoch: 0,
            retry: None,
        }
    }
}
impl<K: Clone + Eq + Send + 'static, V: Send + Sync + 'static> Snapshot<K, V> {
    pub fn clear(&mut self) {
        self.requested = None;
        self.current = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.retry = None;
        // Keep the old worker's gate until it finishes, but reject its result.
    }

    pub fn get(
        &mut self,
        key: K,
        name: &'static str,
        compatible: impl Fn(&K, &K) -> bool,
        load: impl FnOnce(K) -> Option<V> + Send + 'static,
    ) -> Option<Arc<V>> {
        if self.requested.as_ref() != Some(&key) {
            self.requested = Some(key.clone());
            self.retry = None;
        }
        if let Some((epoch, old, rx)) = &self.pending {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                outcome => {
                    if *epoch == self.epoch && old == &key {
                        match outcome {
                            Ok(Some(value)) => self.current = Some((key.clone(), value)),
                            _ => self.retry = Some(Instant::now() + Duration::from_secs(1)),
                        }
                    }
                    self.pending = None;
                }
            }
        }
        if self
            .current
            .as_ref()
            .is_some_and(|(old, _)| !compatible(old, &key))
        {
            self.current = None;
        }
        if self.pending.is_none()
            && self.current.as_ref().is_none_or(|(old, _)| old != &key)
            && self.retry.is_none_or(|until| Instant::now() >= until)
        {
            let (tx, rx) = mpsc::channel();
            self.pending = Some((self.epoch, key.clone(), rx));
            if let Err(error) = std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(key)))
                            .ok()
                            .flatten()
                            .map(Arc::new);
                    let _ = tx.send(result);
                    crate::app::request_repaint();
                })
            {
                self.pending = None;
                self.retry = Some(Instant::now() + Duration::from_secs(1));
                eprintln!("[1kEE] {name}: {error}");
            }
        }
        self.current.as_ref().map(|(_, value)| value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_load_does_not_block_and_reset_rejects_late_result() {
        let mut cache = Snapshot::<u8, u8>::default();
        let (release, wait) = mpsc::channel();
        assert!(
            cache
                .get(
                    1,
                    "snapshot-test",
                    |a, b| a == b,
                    move |_| {
                        wait.recv().unwrap();
                        Some(7)
                    }
                )
                .is_none()
        );
        for _ in 0..100 {
            assert!(
                cache
                    .get(
                        1,
                        "snapshot-test",
                        |a, b| a == b,
                        |_| panic!("duplicate worker")
                    )
                    .is_none()
            );
        }
        cache.clear();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let value = cache.get(2, "snapshot-test", |a, b| a == b, |_| Some(9));
            if let Some(value) = value {
                assert_eq!(*value, 9);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
