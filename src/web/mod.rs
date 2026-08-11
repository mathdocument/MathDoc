mod api;
#[cfg(not(feature = "dev-web"))]
pub mod assets;
pub mod server;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::indcache::IndCache;

const READ_DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct ReadDiscoveryGate {
    last_completed: Option<Instant>,
}

impl ReadDiscoveryGate {
    fn is_due(&self, now: Instant) -> bool {
        self.last_completed.is_none_or(|completed| {
            now.saturating_duration_since(completed) >= READ_DISCOVERY_INTERVAL
        })
    }
}

/// Shared server state handed to every axum handler.
///
/// `IndCache` requires `&mut` for bootstrap/discover/upsert and several derived
/// queries, so it is guarded by a mutex. A separate mutex serializes complete
/// write transactions, including file loading, cycle checks, saves, and reindexing.
/// Read requests share a short discovery gate to coalesce navigation bursts.
#[derive(Clone)]
pub struct AppState {
    cache: Arc<std::sync::Mutex<IndCache>>,
    mutation_lock: Arc<std::sync::Mutex<()>>,
    read_discovery: Arc<std::sync::Mutex<ReadDiscoveryGate>>,
}

impl AppState {
    pub fn new(cache: IndCache) -> Self {
        AppState {
            cache: Arc::new(std::sync::Mutex::new(cache)),
            mutation_lock: Arc::new(std::sync::Mutex::new(())),
            read_discovery: Arc::new(std::sync::Mutex::new(ReadDiscoveryGate::default())),
        }
    }
}
