pub mod api;
pub mod assets;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use crate::indcache::IndCache;

/// Shared server state handed to every axum handler.
///
/// `IndCache` requires `&mut` for bootstrap/discover/upsert and several derived
/// queries (roots, graph check), so it is guarded by a mutex. Handlers lock it
/// for the duration of their work; no handler holds the lock across `.await`.
#[derive(Clone)]
pub struct AppState {
    pub mdcroot: PathBuf,
    pub cache: Arc<std::sync::Mutex<IndCache>>,
}

impl AppState {
    pub fn new(mdcroot: PathBuf, cache: IndCache) -> Self {
        AppState {
            mdcroot,
            cache: Arc::new(std::sync::Mutex::new(cache)),
        }
    }
}
