//! The Gatus feed: live-polls the statuses API into shared state.
//! Rendering lives in `groups` (service boards) and `overview`; this module only ever
//! holds data, so every board reads the same snapshot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::model::{self, Endpoint};

const POLL_INTERVAL: Duration = Duration::from_secs(15);

pub struct GatusSource {
    data: Arc<Mutex<Vec<Endpoint>>>,
    version: Arc<AtomicU64>,
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
        let poll_data = Arc::clone(&data);
        let poll_version = Arc::clone(&version);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL_INTERVAL);
            match model::load(&source) {
                Ok(eps) => {
                    *poll_data.lock().unwrap() = eps;
                    poll_version.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => eprintln!("cksk: poll failed: {e}"),
            }
        });

        GatusSource { data, version }
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
