mod api;
pub mod assets;
pub mod server;

use std::sync::Arc;

use crate::indcache::WorkspaceStore;

/// Shared server state handed to every axum handler.
///
/// `WorkspaceStore` requires `&mut` for bootstrap/discover/upsert and several derived
/// queries, so it is guarded by a mutex. A separate mutex serializes complete
/// write transactions, including file loading, cycle checks, saves, and reindexing.
#[derive(Clone)]
pub struct AppState {
    cache: Arc<std::sync::Mutex<WorkspaceStore>>,
    mutation_lock: Arc<std::sync::Mutex<()>>,
    discovery_started: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}

impl AppState {
    pub fn new(cache: WorkspaceStore) -> Self {
        AppState {
            cache: Arc::new(std::sync::Mutex::new(cache)),
            mutation_lock: Arc::new(std::sync::Mutex::new(())),
            discovery_started: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}
