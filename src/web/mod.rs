pub mod api;
pub mod assets;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use crate::indcache::IndCache;

/// Shared server state handed to every axum handler.
///
/// `IndCache` requires `&mut` for bootstrap/discover/upsert and several derived
/// queries, so it is guarded by a mutex. A separate mutex serializes complete
/// write transactions, including file loading, cycle checks, saves, and reindexing.
#[derive(Clone)]
pub struct AppState {
    pub mdcroot: PathBuf,
    pub cache: Arc<std::sync::Mutex<IndCache>>,
    pub mutation_lock: Arc<std::sync::Mutex<()>>,
}

impl AppState {
    pub fn new(mdcroot: PathBuf, cache: IndCache) -> Self {
        AppState {
            mdcroot,
            cache: Arc::new(std::sync::Mutex::new(cache)),
            mutation_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }
}
