mod api;
pub mod assets;
mod deadline_mutex;
pub mod server;

use std::sync::Arc;

use crate::indcache::WorkspaceStore;
use deadline_mutex::DeadlineMutex;

/// Shared server state handed to every axum handler.
///
/// `WorkspaceStore` requires `&mut` for bootstrap/discover/upsert and several derived
/// queries, so it is guarded by a mutex. A separate mutex serializes complete
/// write transactions, including file loading, cycle checks, saves, and reindexing.
#[derive(Clone)]
pub struct AppState {
    cache: Arc<DeadlineMutex<WorkspaceStore>>,
    mutation_lock: Arc<DeadlineMutex<()>>,
    discovery_started: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}

impl AppState {
    pub fn new(cache: WorkspaceStore) -> Self {
        AppState {
            cache: Arc::new(DeadlineMutex::new(cache)),
            mutation_lock: Arc::new(DeadlineMutex::new(())),
            discovery_started: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}
