//! The Gatus feed: live-polls the statuses API into shared state.
//! Rendering lives in `groups` (service boards) and `overview`; this module only ever
//! holds data, so every board reads the same snapshot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::model::{self, Endpoint};

const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Past this data age the boards say so instead of rendering stale green.
const STALE_AFTER_SECS: u64 = 90;

pub struct GatusSource {
    data: Arc<Mutex<Vec<Endpoint>>>,
    version: Arc<AtomicU64>,
    updated: Arc<Mutex<Option<Instant>>>,
}

impl GatusSource {
    /// Loads `source` once synchronously (so the very first render has data), then spawns a
    /// background thread that re-polls it every 15s into the shared `Mutex`. A failed poll
    /// (network hiccup, bad JSON) logs to stderr and leaves the previous data in place -- an
    /// endpoint list is more useful stale than replaced by nothing.
    pub fn new(source: String) -> Self {
        let initial = model::load(&source).unwrap_or_else(|e| {
            eprintln!("cksk: initial load failed: {e}");
            Vec::new()
        });
        eprintln!("cksk: loaded {} endpoints from {source}", initial.len());

        let data = Arc::new(Mutex::new(initial));
        let version = Arc::new(AtomicU64::new(0));
        let updated = Arc::new(Mutex::new(Some(Instant::now())));
        let poll_data = Arc::clone(&data);
        let poll_version = Arc::clone(&version);
        let poll_updated = Arc::clone(&updated);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL_INTERVAL);
            match model::load(&source) {
                Ok(eps) => {
                    *poll_data.lock().unwrap() = eps;
                    poll_version.fetch_add(1, Ordering::Relaxed);
                    *poll_updated.lock().unwrap() = Some(Instant::now());
                }
                Err(e) => eprintln!("cksk: poll failed: {e}"),
            }
        });

        GatusSource {
            data,
            version,
            updated,
        }
    }

    /// Seconds since the last successful poll, if any poll ever succeeded.
    pub fn data_age_secs(&self) -> Option<u64> {
        self.updated.lock().unwrap().map(|t| t.elapsed().as_secs())
    }

    /// True when the data is too old to trust at a glance.
    pub fn is_stale(&self) -> bool {
        self.data_age_secs()
            .is_none_or(|age| age > STALE_AFTER_SECS)
    }

    /// Current data version -- compare against a previously observed value to detect a refresh.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// One cheap clone of the current endpoint list per frame; 64 small structs at 1fps.
    pub fn snapshot(&self) -> Vec<Endpoint> {
        self.data.lock().unwrap().clone()
    }

    pub fn counts(eps: &[Endpoint]) -> (usize, usize, usize) {
        let mut up = 0;
        let mut down = 0;
        let mut unk = 0;
        for e in eps {
            match e.up() {
                Some(true) => up += 1,
                Some(false) => down += 1,
                None => unk += 1,
            }
        }
        (up, down, unk)
    }
}
