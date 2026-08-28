mod derived;
mod discovery;
mod queries;
mod refresh;
mod schema;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::core::{
    short_fnode, DependencyCandidates, DependencyItem, DependencyTraversalReport,
    FormalizationStatus, GraphCheckReport, GraphIssue, GraphRootItem, NodeDegrees, NodeSummary,
};
use crate::mdocnode::{MdocHead, MdocIdentity, MdocNode};

#[derive(Debug, thiserror::Error)]
pub enum ResolveRefError {
    #[error("mdoc reference cannot be empty")]
    Empty,
    #[error("no mdoc matched reference: {0}")]
    NotFound(String),
    #[error("ambiguous mdoc reference '{reference}', matches: {matches}")]
    Ambiguous { reference: String, matches: String },
    #[error("invalid mdoc file: {0}")]
    Invalid(String),
}

fn regular_file_identity(path: &Path, name: &str) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {name} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
        bail!(
            "{name} is not one regular single-link file: {}",
            path.display()
        );
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn regular_directory_identity(path: &Path, name: &str) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {name} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(crate::workspace::WorkspaceGenerationError::new(format!(
            "{name} is not a real directory: {}",
            path.display()
        ))
        .into());
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn open_database_guard(path: &Path) -> Result<(std::fs::File, (u64, u64))> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("opening index database guard {}", path.display()))?;
    let metadata = file.metadata()?;
    let identity = regular_file_identity(path, "index database")?;
    use std::os::unix::fs::MetadataExt;
    if (metadata.dev(), metadata.ino()) != identity {
        bail!(
            "index database changed while it was being opened: {}",
            path.display()
        );
    }
    Ok((file, identity))
}

fn connection_database_has_moved(connection: &Connection) -> Result<bool> {
    let mut moved = 0_i32;
    // SAFETY: `connection` owns a live SQLite handle, the database name is
    // NUL-terminated, and SQLite writes an integer to `moved` synchronously.
    let code = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::from_mut(&mut moved).cast(),
        )
    };
    if code != rusqlite::ffi::SQLITE_OK {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("checking whether the index database moved".to_string()),
        )
        .into());
    }
    Ok(moved != 0)
}

/// SQLite-backed index of a MathDoc workspace.
pub struct WorkspaceStore {
    root: PathBuf,
    control_identity: (u64, u64),
    db_path: PathBuf,
    db_identity: (u64, u64),
    db_file: std::fs::File,
    conn: Connection,
}

/// Existing public name retained for callers that use the workspace as a read cache.
pub type IndCache = WorkspaceStore;

pub(crate) struct MutationSession<'store, 'lock> {
    store: &'store mut WorkspaceStore,
    lock: &'lock crate::workspace::WorkspaceMutationLock,
    dirty: bool,
}

pub(crate) struct WorkdraftObservationCache {
    pub(crate) valid_mdocs: usize,
    pub(crate) source_files: usize,
    files: HashMap<
        String,
        [Option<crate::workspace::FileStatSnapshot>; crate::config::BUILTIN_SRCTYPE_COUNT + 1],
    >,
}

impl WorkdraftObservationCache {
    pub(crate) fn file_count(&self) -> usize {
        self.files
            .values()
            .flat_map(|observations| observations.iter())
            .filter(|stat| stat.is_some())
            .count()
    }

    pub(crate) fn stat(
        &self,
        source_id: &str,
        srctype: &str,
    ) -> Option<&crate::workspace::FileStatSnapshot> {
        let slot = workdraft_observation_slot(srctype)?;
        self.files.get(source_id)?.get(slot)?.as_ref()
    }
}

fn workdraft_observation_slot(srctype: &str) -> Option<usize> {
    if srctype.is_empty() {
        Some(0)
    } else {
        crate::config::builtin_srctype_index(srctype).map(|index| index + 1)
    }
}

pub(crate) struct WorkdraftObservation {
    pub(crate) source_id: String,
    pub(crate) srctype: String,
    pub(crate) stat: Option<crate::workspace::FileStatSnapshot>,
}

impl WorkspaceStore {
    /// Open (or create) the index database for the workspace rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        let _profile = crate::profile::scope("IndCache::open");
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
        Self::open_under_mutation_lock_with(&mutation_lock, false)
    }

    pub(crate) fn open_refreshed(root: PathBuf) -> Result<Self> {
        let _profile = crate::profile::scope("IndCache::open_refreshed");
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
        Self::open_under_mutation_lock_with(&mutation_lock, true)
    }

    pub(crate) fn open_under_mutation_lock(
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<Self> {
        Self::open_under_mutation_lock_with(mutation_lock, false)
    }

    pub(crate) fn open_refreshed_under_mutation_lock(
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<Self> {
        Self::open_under_mutation_lock_with(mutation_lock, true)
    }

    fn open_under_mutation_lock_with(
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        refresh: bool,
    ) -> Result<Self> {
        let root = mutation_lock.root()?.to_path_buf();
        let control_identity = mutation_lock.control_identity()?;
        let db_path = root.join(".mdc").join("index.db");
        let (db_file, db_identity) = open_database_guard(&db_path)?;
        crate::workspace::run_test_hook(crate::workspace::TestHookPoint::IndexBeforeConnectionOpen);
        let conn = schema::open_db(&db_path)?;
        crate::workspace::run_test_hook(crate::workspace::TestHookPoint::IndexAfterConnectionOpen);
        if regular_file_identity(&db_path, "index database")? != db_identity {
            return Err(crate::workspace::WorkspaceGenerationError::new(format!(
                "index database changed while SQLite opened it: {}",
                db_path.display()
            ))
            .into());
        }
        if connection_database_has_moved(&conn)? {
            return Err(crate::workspace::WorkspaceGenerationError::new(format!(
                "SQLite opened a displaced index database generation: {}",
                db_path.display()
            ))
            .into());
        }
        let mut cache = IndCache {
            root,
            control_identity,
            db_path,
            db_identity,
            db_file,
            conn,
        };
        cache.validate_mutation_lock(mutation_lock)?;
        if cache.index_is_dirty()? {
            cache.recover_index()?;
        } else if refresh {
            cache.refresh_all()?;
        } else {
            cache.bootstrap_if_needed()?;
        }
        cache.validate_mutation_lock(mutation_lock)?;
        Ok(cache)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn load_workdraft_observations(
        &self,
        manifest_digest: &[u8; 32],
    ) -> Result<Option<WorkdraftObservationCache>> {
        self.require_current_database()?;
        let (stored_digest, valid_mdocs, source_files): (Vec<u8>, i64, i64) = self.conn.query_row(
            "SELECT manifest_digest, valid_mdocs, source_files
                 FROM mdoc_workdraft_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if stored_digest.as_slice() != manifest_digest {
            return Ok(None);
        }
        if valid_mdocs < 0 || source_files < 0 {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(
            "SELECT source_id, srctype, present, device, inode, size,
                    mtime, mtime_nsec, ctime, ctime_nsec, mode, uid, gid
             FROM mdoc_workdraft_observations",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let source_id: String = row.get(0)?;
                let srctype: String = row.get(1)?;
                let present: bool = row.get(2)?;
                let stat = crate::workspace::FileStatSnapshot {
                    device: row.get::<_, i64>(3)? as u64,
                    inode: row.get::<_, i64>(4)? as u64,
                    size: row.get::<_, i64>(5)? as u64,
                    mtime: row.get(6)?,
                    mtime_nsec: row.get(7)?,
                    ctime: row.get(8)?,
                    ctime_nsec: row.get(9)?,
                    mode: row.get::<_, i64>(10)? as u32,
                    uid: row.get::<_, i64>(11)? as u32,
                    gid: row.get::<_, i64>(12)? as u32,
                };
                Ok(((source_id, srctype), present, stat))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.iter().any(|(_, present, _)| !present) {
            return Ok(None);
        }
        let mut files = HashMap::new();
        for ((source_id, srctype), _, stat) in rows {
            let Some(slot) = workdraft_observation_slot(&srctype) else {
                return Ok(None);
            };
            let observations = files
                .entry(source_id)
                .or_insert([None; crate::config::BUILTIN_SRCTYPE_COUNT + 1]);
            if observations[slot].replace(stat).is_some() {
                return Ok(None);
            }
        }
        self.require_current_database()?;
        Ok(Some(WorkdraftObservationCache {
            valid_mdocs: valid_mdocs as usize,
            source_files: source_files as usize,
            files,
        }))
    }

    pub(crate) fn index_is_dirty(&self) -> Result<bool> {
        self.require_current_database()?;
        let dirty = self.conn.query_row(
            "SELECT index_dirty FROM mdoc_index_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        self.require_current_database()?;
        Ok(dirty)
    }

    fn set_index_dirty(&mut self, dirty: bool) -> Result<()> {
        self.require_current_database()?;
        self.conn.execute(
            "UPDATE mdoc_index_state SET index_dirty = ? WHERE id = 1",
            [dirty],
        )?;
        self.require_current_database()?;
        Ok(())
    }

    pub(crate) fn recover_index(&mut self) -> Result<()> {
        self.require_current_database()?;
        self.conn.execute(
            "UPDATE mdoc_index_state SET index_digest = '' WHERE id = 1",
            [],
        )?;
        self.refresh_all()?;
        self.set_index_dirty(false)
    }

    pub(crate) fn store_workdraft_observations(
        &mut self,
        manifest_digest: &[u8; 32],
        valid_mdocs: usize,
        source_files: usize,
        observations: Vec<WorkdraftObservation>,
    ) -> Result<()> {
        self.write_workdraft_observations(
            manifest_digest,
            valid_mdocs,
            source_files,
            observations,
            true,
        )
    }

    pub(crate) fn update_workdraft_observations(
        &mut self,
        manifest_digest: &[u8; 32],
        valid_mdocs: usize,
        source_files: usize,
        observations: Vec<WorkdraftObservation>,
    ) -> Result<()> {
        self.write_workdraft_observations(
            manifest_digest,
            valid_mdocs,
            source_files,
            observations,
            false,
        )
    }

    fn write_workdraft_observations(
        &mut self,
        manifest_digest: &[u8; 32],
        valid_mdocs: usize,
        source_files: usize,
        observations: Vec<WorkdraftObservation>,
        replace_all: bool,
    ) -> Result<()> {
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        if replace_all {
            tx.execute("DELETE FROM mdoc_workdraft_observations", [])?;
        } else {
            let source_ids = observations
                .iter()
                .map(|observation| observation.source_id.as_str())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for chunk in source_ids.chunks(queries::CHUNK_SIZE) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                tx.execute(
                    &format!(
                        "DELETE FROM mdoc_workdraft_observations
                         WHERE source_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(chunk.iter().copied()),
                )?;
            }
        }
        {
            let mut write = tx.prepare(
                "INSERT INTO mdoc_workdraft_observations
                   (source_id, srctype, present, device, inode, size,
                    mtime, mtime_nsec, ctime, ctime_nsec, mode, uid, gid)
                 VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for observation in observations {
                let Some(stat) = observation.stat else {
                    continue;
                };
                write.execute(rusqlite::params![
                    observation.source_id,
                    observation.srctype,
                    stat.device as i64,
                    stat.inode as i64,
                    stat.size as i64,
                    stat.mtime,
                    stat.mtime_nsec,
                    stat.ctime,
                    stat.ctime_nsec,
                    i64::from(stat.mode),
                    i64::from(stat.uid),
                    i64::from(stat.gid),
                ])?;
            }
        }
        tx.execute(
            "UPDATE mdoc_workdraft_state
             SET manifest_digest = ?, valid_mdocs = ?, source_files = ?
             WHERE id = 1",
            rusqlite::params![
                manifest_digest.as_slice(),
                i64::try_from(valid_mdocs).unwrap_or(i64::MAX),
                i64::try_from(source_files).unwrap_or(i64::MAX),
            ],
        )?;
        tx.commit()?;
        self.require_current_database()?;
        Ok(())
    }

    fn require_current_database(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;

        let control_path = self.root.join(".mdc");
        let current_control = regular_directory_identity(&control_path, "workspace control path")
            .map_err(|error| {
            crate::workspace::WorkspaceGenerationError::new(format!(
                "workspace control directory generation is uncertain at {}: {error}",
                control_path.display()
            ))
        })?;
        if current_control != self.control_identity {
            return Err(crate::workspace::WorkspaceGenerationError::new(format!(
                "workspace control directory changed while the cache was open: {}",
                control_path.display()
            ))
            .into());
        }
        let opened = self.db_file.metadata()?;
        let current = regular_file_identity(&self.db_path, "index database").map_err(|error| {
            crate::workspace::WorkspaceGenerationError::new(format!(
                "workspace index database generation is uncertain at {}: {error}",
                self.db_path.display()
            ))
        })?;
        if (opened.dev(), opened.ino()) == self.db_identity
            && current == self.db_identity
            && !connection_database_has_moved(&self.conn)?
        {
            Ok(())
        } else {
            Err(crate::workspace::WorkspaceGenerationError::new(format!(
                "workspace index database changed while the cache was open: {}",
                self.db_path.display()
            ))
            .into())
        }
    }

    fn with_current_database<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        self.require_current_database()?;
        let result = operation(&self.conn);
        self.require_current_database()?;
        result
    }

    pub(crate) fn acquire_mutation_lock(&self) -> Result<crate::workspace::WorkspaceMutationLock> {
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&self.root)?;
        self.validate_mutation_lock(&mutation_lock)?;
        Ok(mutation_lock)
    }

    pub(crate) fn validate_mutation_lock(
        &self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<()> {
        mutation_lock.validate_identity(&self.root, self.control_identity)?;
        self.require_current_database()
    }

    pub(crate) fn mutation_session<'store, 'lock>(
        &'store mut self,
        mutation_lock: &'lock crate::workspace::WorkspaceMutationLock,
    ) -> Result<MutationSession<'store, 'lock>> {
        self.validate_mutation_lock(mutation_lock)?;
        Ok(MutationSession {
            store: self,
            lock: mutation_lock,
            dirty: false,
        })
    }

    pub(crate) fn mutate<R>(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        operation: impl FnOnce(&mut MutationSession<'_, '_>) -> Result<R>,
    ) -> Result<R> {
        let mut mutation = self.mutation_session(mutation_lock)?;
        match operation(&mut mutation) {
            Ok(value) => match mutation.commit() {
                Ok(()) => Ok(value),
                Err(error) => Err(crate::workspace::PersistenceRecoveryError::from_attempts(
                    error,
                    Ok(()),
                    mutation.abort(),
                    "roll back workspace mutation",
                    "repair the workspace index",
                )),
            },
            Err(error) => Err(crate::workspace::PersistenceRecoveryError::from_attempts(
                error,
                Ok(()),
                mutation.abort(),
                "roll back workspace mutation",
                "repair the workspace index",
            )),
        }
    }

    // ── Bootstrap / refresh ──────────────────────────────────────────────────

    /// Bootstrap the index on first use; no-op if already bootstrapped.
    fn bootstrap_if_needed(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::bootstrap_if_needed");
        self.require_current_database()?;
        if !queries::is_bootstrapped(&self.conn)? {
            let tx = self.conn.transaction()?;
            let formal_validation = refresh::refresh_search_index(&tx, &self.root)?;
            let _commit = crate::profile::scope("sqlite::bootstrap_commit");
            tx.commit()?;
            self.validate_formal_status_commit(formal_validation)?;
        } else {
            self.refresh_formal_statuses()?;
        }
        Ok(())
    }

    /// Full workspace rescan; rebuilds the entire index.
    pub fn refresh_all(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::refresh_all");
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        let formal_validation = refresh::refresh_search_index(&tx, &self.root)?;
        let _commit = crate::profile::scope("sqlite::refresh_all_commit");
        tx.commit()?;
        self.validate_formal_status_commit(formal_validation)
    }

    /// Discover additions, deletions, and metadata changes using the metadata fast path.
    pub fn discover_workspace_changes(&mut self) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::discover_workspace_changes");
        self.require_current_database()?;
        let changes = discovery::discover_workspace_changes(&self.conn, &self.root)?;
        if changes.is_empty() {
            self.require_current_database()?;
            return Ok(());
        }
        crate::workspace::run_test_hook(crate::workspace::TestHookPoint::DiscoveryBeforeApply);
        let tx = self.conn.transaction()?;
        let (changed_fnodes, has_deletion) =
            discovery::apply_workspace_changes(&tx, &self.root, changes)?;
        if has_deletion {
            // Deletions can decrease ancestor depths; full backfill is needed.
            derived::backfill_all_topo_depths(&tx)?;
        } else {
            derived::refresh_topo_depth_upward_from_many(&tx, &changed_fnodes)?;
        }
        let formal_validation = crate::formal::status::refresh_index_statuses(&tx, &self.root)?;
        tx.commit()?;
        self.validate_formal_status_commit(formal_validation)
    }

    /// Upsert a single file path and update its topo depths.
    pub fn upsert_path(&mut self, file_path: &Path) -> Result<()> {
        self.upsert_paths(&[file_path.to_path_buf()])
    }

    /// Upsert known file paths in one transaction and refresh their shared ancestors once.
    pub(crate) fn upsert_paths(&mut self, file_paths: &[PathBuf]) -> Result<()> {
        self.require_current_database()?;
        if file_paths.is_empty() {
            return Ok(());
        }
        let mut seen = HashSet::with_capacity(file_paths.len());
        let mut resolved_paths = Vec::with_capacity(file_paths.len());
        for file_path in file_paths {
            let resolved = crate::workspace::resolve_mdoc_path(&self.root, file_path)?;
            if seen.insert(resolved.clone()) {
                resolved_paths.push(resolved);
            }
        }
        let tx = self.conn.transaction()?;
        let mut changed_fnodes = HashSet::new();
        let mut needs_full_topo_backfill = false;
        for file_path in resolved_paths {
            let outcome = refresh::upsert_mdoc_row(&tx, &self.root, &file_path)?;
            if outcome.graph_changed {
                if outcome.old_fnode.is_none() && outcome.new_fnode.is_none() {
                    needs_full_topo_backfill = true;
                }
                changed_fnodes.extend(outcome.old_fnode);
                changed_fnodes.extend(outcome.new_fnode);
            }
        }
        if needs_full_topo_backfill {
            derived::backfill_all_topo_depths(&tx)?;
        } else {
            derived::refresh_topo_depth_upward_from_many(&tx, &changed_fnodes)?;
        }
        let formal_validation = crate::formal::status::refresh_index_statuses(&tx, &self.root)?;

        tx.commit()?;
        self.validate_formal_status_commit(formal_validation)
    }

    /// Create a node and index it as one recoverable operation.
    pub(crate) fn create_node(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        node: &MdocNode,
    ) -> Result<crate::workspace::AppliedWrite> {
        self.mutate(mutation_lock, |mutation| mutation.create_node(node))
    }

    /// Replace a node and update its index entry as one recoverable operation.
    pub(crate) fn replace_node(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        node: &MdocNode,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<()> {
        self.mutate(mutation_lock, |mutation| {
            mutation.replace_node(node, snapshot)
        })
    }

    fn validate_node_path(&self, node: &MdocNode) -> Result<PathBuf> {
        crate::workspace::resolve_mdoc_path(&self.root, &node.path)
    }

    fn write_and_index_node(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        path: &Path,
        payload: &[u8],
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<crate::workspace::AppliedWrite> {
        self.validate_mutation_lock(mutation_lock)?;
        let created = matches!(snapshot, crate::workspace::FileSnapshot::Missing);
        let applied = match snapshot.replace_beneath(&self.root, path, payload) {
            Ok(applied) => applied,
            Err(error) => {
                let index_repair = self
                    .validate_mutation_lock(mutation_lock)
                    .and_then(|_| self.recover_index());
                return Err(crate::workspace::PersistenceRecoveryError::from_attempts(
                    error,
                    Ok(()),
                    index_repair,
                    &format!("restore {}", path.display()),
                    &format!("repair the index for {}", path.display()),
                ));
            }
        };
        let index_error = match self.upsert_path(path) {
            Ok(()) => {
                crate::workspace::run_test_hook(
                    crate::workspace::TestHookPoint::IndexAfterNodeUpsert,
                );
                self.validate_mutation_lock(mutation_lock)
                    .with_context(|| {
                        format!(
                            "node write and index committed under an uncertain lock: {}",
                            path.display()
                        )
                    })?;
                match applied.require_current().with_context(|| {
                    format!(
                        "node file changed after its index update: {}",
                        path.display()
                    )
                }) {
                    Ok(()) => {
                        return Ok(applied);
                    }
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };

        let lock_validation = self.validate_mutation_lock(mutation_lock).with_context(|| {
            format!(
                "index update failed after the mutation lock changed: {}",
                path.display()
            )
        });
        let rollback_result = applied.rollback();
        let restore_index_result = match lock_validation {
            Ok(()) => self
                .upsert_path(path)
                .and_then(|_| self.validate_mutation_lock(mutation_lock))
                .and_then(|_| self.set_index_dirty(false)),
            Err(error) => Err(error),
        };
        let rollback_action = if created { "remove" } else { "restore" };
        Err(crate::workspace::PersistenceRecoveryError::from_attempts(
            index_error,
            rollback_result,
            restore_index_result,
            &format!("{rollback_action} {}", path.display()),
            &format!("repair the index for {}", path.display()),
        ))
    }

    /// Upsert all dependencies reachable from `root_path` up to `depth` hops (-1 = infinite).
    pub fn refresh_reachable_from_path(&mut self, root_path: &Path, depth: i32) -> Result<()> {
        let _profile = crate::profile::scope("IndCache::refresh_reachable_from_path");
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        let upserted_fnodes = {
            let _phase = crate::profile::scope("refresh::reachable_upserts");
            refresh::refresh_reachable_from_path(&tx, &self.root, root_path, depth)?
        };
        // Incremental topo update for each upserted fnode. Weak components are
        // rebuilt lazily from the dirty flag when roots are queried.
        {
            let _phase = crate::profile::scope("derived::refresh_reachable_topo");
            derived::refresh_topo_depth_upward_from_many(&tx, &upserted_fnodes)?;
        }
        let formal_validation = crate::formal::status::refresh_index_statuses(&tx, &self.root)?;
        let _commit = crate::profile::scope("sqlite::refresh_reachable_commit");
        tx.commit()?;
        self.validate_formal_status_commit(formal_validation)
    }

    // ── Read queries ─────────────────────────────────────────────────────────

    pub fn count(&self) -> Result<u32> {
        self.with_current_database(queries::mdoc_count)
    }

    pub fn path_has_blocking_issue(&self, rel_path: &str) -> Result<bool> {
        self.with_current_database(|connection| {
            queries::path_has_blocking_issue(connection, rel_path)
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<NodeSummary>> {
        self.with_current_database(|connection| queries::search(connection, query, limit))
    }

    pub fn all_node_summaries(&self) -> Result<Vec<NodeSummary>> {
        self.with_current_database(queries::all_node_summaries)
    }

    pub fn dependency_candidates(
        &self,
        source_fnode: &str,
        query: &str,
        limit: usize,
    ) -> Result<DependencyCandidates> {
        self.with_current_database(|connection| {
            queries::dependency_candidates(connection, source_fnode, query, limit)
        })
    }

    pub fn node_summary(&self, fnode: &str) -> Result<NodeSummary> {
        self.with_current_database(|connection| queries::node_summary(connection, fnode))
    }

    pub fn node_degrees(&self, fnode: &str) -> Result<NodeDegrees> {
        self.with_current_database(|connection| queries::node_degrees(connection, fnode))
    }

    pub fn formalization_status(&mut self, fnode: &str) -> Result<FormalizationStatus> {
        let has_attestation = crate::formal::attestation::load_for_status(&self.root)
            .is_ok_and(|loaded| loaded.manifest.has_attestation_for(fnode));
        if !has_attestation {
            let rel_path = self.with_current_database(|connection| {
                queries::path_for_fnode_if_unique(connection, fnode)
            })?;
            if let Some(rel_path) = rel_path {
                let path = self.root.join(rel_path);
                self.upsert_path(&path)?;
            }
        }
        self.indexed_formalization_status(fnode)
    }

    pub(crate) fn indexed_formalization_status(&self, fnode: &str) -> Result<FormalizationStatus> {
        self.with_current_database(|connection| queries::formalization_status(connection, fnode))
    }

    pub(crate) fn refresh_formal_statuses(&mut self) -> Result<()> {
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        let formal_validation = crate::formal::status::refresh_index_statuses(&tx, &self.root)?;
        tx.commit()?;
        self.validate_formal_status_commit(formal_validation)
    }

    fn validate_formal_status_commit(
        &mut self,
        validation: crate::formal::status::FormalStatusValidation,
    ) -> Result<()> {
        self.require_current_database()?;
        let Err(error) = validation.ensure_current() else {
            return Ok(());
        };
        let repair = (|| {
            self.require_current_database()?;
            let tx = self.conn.transaction()?;
            crate::formal::status::downgrade_verified_statuses(&tx)?;
            tx.commit()?;
            self.require_current_database()
        })();
        Err(crate::workspace::PersistenceRecoveryError::from_attempts(
            error,
            Ok(()),
            repair,
            "discard formal status validation",
            "downgrade formal statuses",
        ))
    }

    pub(crate) fn invalidate_formal_attestations(
        &mut self,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        fnode: &str,
        languages: &[String],
    ) -> Result<()> {
        self.validate_mutation_lock(mutation_lock)?;
        let mut loaded = crate::formal::attestation::load(&self.root)?;
        for language in languages {
            loaded.manifest.remove(fnode, language)?;
        }
        self.validate_mutation_lock(mutation_lock)?;
        crate::formal::attestation::save(&self.root, loaded)?;
        // Revocation is fail-safe: never restore a credential because the derived
        // SQLite status could not be refreshed.
        let refresh_result = self.refresh_formal_statuses();
        let lock_result = self.validate_mutation_lock(mutation_lock);
        refresh_result?;
        lock_result
    }

    pub(crate) fn publish_formal_attestations(
        &mut self,
        work_lock: &crate::workspace::WorkspaceWorkLock,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        expected_manifest: &crate::workspace::FileSnapshot,
        fnode: &str,
        outcomes: &[(
            String,
            bool,
            Option<crate::formal::FormalCompilationReceipt>,
        )],
    ) -> Result<Vec<(String, String)>> {
        self.validate_formal_publication_locks(work_lock, mutation_lock)?;
        crate::formal::attestation::require_snapshot_current(&self.root, expected_manifest)?;
        let mut loaded = crate::formal::attestation::load(&self.root)?;
        crate::formal::attestation::require_snapshot_current(&self.root, expected_manifest)?;
        let mut errors = Vec::new();
        let mut prepared = Vec::new();
        for (language, succeeded, receipt) in outcomes {
            loaded.manifest.remove(fnode, language)?;
            if !succeeded {
                continue;
            }
            let Some(receipt) = receipt else {
                errors.push((
                    language.clone(),
                    "successful formal compiler returned no compilation receipt".to_string(),
                ));
                continue;
            };
            match crate::formal::status::prepare_attestation(
                &self.conn,
                &self.root,
                &loaded.manifest,
                fnode,
                language,
                receipt,
            ) {
                Ok(attestation) => {
                    loaded.manifest.set(fnode, language, attestation)?;
                    prepared.push(language.clone());
                }
                Err(error) => errors.push((language.clone(), error.to_string())),
            }
        }
        self.commit_formal_manifest(work_lock, mutation_lock, loaded)?;
        let status = match self.formalization_status(fnode) {
            Ok(status) => status,
            Err(error) => {
                self.invalidate_formal_attestations(mutation_lock, fnode, &prepared)
                    .context("removing attestations after status validation failed")?;
                return Err(error);
            }
        };
        let mut failed_prepared = Vec::new();
        for language in &prepared {
            let verified = match language.as_str() {
                "lean" => status.lean == crate::core::FormalCodeStatus::Verified,
                "rocq" => status.rocq == crate::core::FormalCodeStatus::Verified,
                _ => false,
            };
            if !verified {
                errors.push((
                    language.clone(),
                    "published attestation did not produce a verified status".to_string(),
                ));
                failed_prepared.push(language.clone());
            }
        }
        if !failed_prepared.is_empty() {
            self.invalidate_formal_attestations(mutation_lock, fnode, &failed_prepared)?;
        }
        self.validate_formal_publication_locks(work_lock, mutation_lock)?;
        Ok(errors)
    }

    fn commit_formal_manifest(
        &mut self,
        work_lock: &crate::workspace::WorkspaceWorkLock,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
        loaded: crate::formal::attestation::LoadedManifest,
    ) -> Result<()> {
        self.validate_formal_publication_locks(work_lock, mutation_lock)?;
        let applied = crate::formal::attestation::save(&self.root, loaded)?;
        let commit_result = self
            .refresh_formal_statuses()
            .and_then(|_| self.validate_formal_publication_locks(work_lock, mutation_lock));
        let Err(error) = commit_result else {
            return Ok(());
        };
        let Some(applied) = applied else {
            return Err(error);
        };
        Err(crate::workspace::PersistenceRecoveryError::from_attempts(
            error,
            applied.rollback(),
            self.refresh_formal_statuses(),
            "restore the formal attestation manifest",
            "repair formal statuses",
        ))
    }

    fn validate_formal_publication_locks(
        &self,
        work_lock: &crate::workspace::WorkspaceWorkLock,
        mutation_lock: &crate::workspace::WorkspaceMutationLock,
    ) -> Result<()> {
        self.validate_mutation_lock(mutation_lock)?;
        work_lock.validate_identity(&self.root, self.control_identity)
    }

    #[cfg(test)]
    fn exact_fnode_rows(&self, fnode: &str) -> Result<Vec<(String, String, String)>> {
        self.with_current_database(|connection| queries::exact_fnode_rows(connection, fnode))
    }

    pub fn reconcile_fnode_paths(&mut self, fnode: &str) -> Result<Vec<PathBuf>> {
        let mut paths = self.reconcile_fnode_paths_many(&[fnode])?;
        Ok(paths
            .remove(fnode)
            .expect("single-fnode reconciliation returns its requested key"))
    }

    pub(crate) fn reconcile_fnode_paths_many(
        &mut self,
        fnodes: &[&str],
    ) -> Result<HashMap<String, Vec<PathBuf>>> {
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        let mut paths_by_fnode = HashMap::with_capacity(fnodes.len());
        let mut changed_fnodes = HashSet::new();
        for &fnode in fnodes {
            if paths_by_fnode.contains_key(fnode) {
                continue;
            }
            let rows = queries::exact_fnode_rows(&tx, fnode)?;
            let mut paths = Vec::new();
            for (_, _, rel_path) in rows {
                match refresh::current_cached_mdoc_path(&self.root, &rel_path)? {
                    Some(path) => paths.push(path),
                    None => {
                        refresh::delete_indexed_path(&tx, &rel_path)?;
                        changed_fnodes.insert(fnode.to_string());
                    }
                }
            }
            paths_by_fnode.insert(fnode.to_string(), paths);
        }
        let formal_validation = if changed_fnodes.is_empty() {
            None
        } else {
            derived::refresh_topo_depth_upward_from_many(&tx, &changed_fnodes)?;
            Some(crate::formal::status::refresh_index_statuses(
                &tx, &self.root,
            )?)
        };
        tx.commit()?;
        if let Some(formal_validation) = formal_validation {
            self.validate_formal_status_commit(formal_validation)?;
        } else {
            self.require_current_database()?;
        }
        Ok(paths_by_fnode)
    }

    pub fn lookup_by_fnode(&self, fnodes: &[&str]) -> Result<HashMap<String, (String, String)>> {
        self.with_current_database(|connection| queries::lookup_by_fnode(connection, fnodes))
    }

    pub fn issue_for_fnode(&self, fnode: &str) -> Result<Option<GraphIssue>> {
        self.with_current_database(|connection| queries::issue_for_fnode(connection, fnode))
    }

    pub fn ref_item_for_fnode(&self, fnode: &str, depth: u32) -> Result<DependencyItem> {
        self.with_current_database(|connection| {
            queries::ref_item_for_fnode(connection, fnode, depth)
        })
    }

    pub(crate) fn ref_items_for_fnodes(
        &self,
        fnodes: &[String],
        depth: u32,
    ) -> Result<Vec<DependencyItem>> {
        let fnodes: Vec<&str> = fnodes.iter().map(String::as_str).collect();
        self.with_current_database(|connection| {
            queries::ref_items_for_fnodes(connection, &fnodes, depth)
        })
    }

    pub fn referrer_items(&self, target_fnode: &str, depth: i32) -> Result<Vec<DependencyItem>> {
        self.with_current_database(|connection| {
            queries::referrer_items(connection, target_fnode, depth)
        })
    }

    pub fn direct_referrer_summaries(&self, fnode: &str) -> Result<Vec<NodeSummary>> {
        self.with_current_database(|connection| {
            queries::direct_referrer_summaries(connection, fnode)
        })
    }

    pub fn direct_dependency_summaries(&self, fnode: &str) -> Result<Vec<NodeSummary>> {
        self.with_current_database(|connection| {
            queries::direct_dependency_summaries(connection, fnode)
        })
    }

    /// All dependency edges whose source document has no blocking issue.
    pub fn all_valid_edges(&self) -> Result<Vec<(String, String)>> {
        self.with_current_database(queries::all_valid_edges)
    }

    pub fn is_reachable(&self, from_fnode: &str, to_fnode: &str) -> Result<bool> {
        self.with_current_database(|connection| {
            queries::is_reachable(connection, from_fnode, to_fnode)
        })
    }

    pub(crate) fn reverse_reachable_fnodes(&self, target_fnode: &str) -> Result<HashSet<String>> {
        self.with_current_database(|connection| {
            queries::reverse_reachable_fnodes(connection, target_fnode)
        })
    }

    pub fn dependency_report(
        &self,
        root_fnode: &str,
        depth: i32,
    ) -> Result<DependencyTraversalReport> {
        self.with_current_database(|connection| {
            queries::dependency_report(connection, root_fnode, depth)
        })
    }

    pub fn leaf_dependency_report(&self, root_fnode: &str) -> Result<DependencyTraversalReport> {
        self.with_current_database(|connection| {
            queries::leaf_dependency_report(connection, root_fnode)
        })
    }

    // ── Write-then-read (need &mut for transaction) ───────────────────────────

    pub fn global_root_items(&mut self) -> Result<Vec<GraphRootItem>> {
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        derived::ensure_weak_components(&tx)?;
        let result = queries::global_root_items(&tx)?;
        tx.commit()?;
        self.require_current_database()?;
        Ok(result)
    }

    pub fn graph_check_report(&mut self) -> Result<GraphCheckReport> {
        let _profile = crate::profile::scope("IndCache::graph_check_report");
        self.require_current_database()?;
        let tx = self.conn.transaction()?;
        let cycles = derived::ensure_scc_cache(&tx)?;
        let result = queries::graph_check_report(&tx, cycles)?;
        tx.commit()?;
        self.require_current_database()?;
        Ok(result)
    }

    // ── Reference resolution ─────────────────────────────────────────────────

    /// Resolve a reference string to `(fnode, title, abs_path)`.
    ///
    /// The reference may be:
    /// - A path-like string (contains `/`, ends in `.mdoc`, or starts with `.`)
    /// - An fnode or fnode prefix
    pub fn resolve_ref(
        &self,
        raw_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(String, String, PathBuf)> {
        self.with_current_database(|_| self.resolve_ref_inner(raw_ref, cwd))
    }

    fn resolve_ref_inner(
        &self,
        raw_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(String, String, PathBuf)> {
        let raw_ref = raw_ref.trim();
        if raw_ref.is_empty() {
            return Err(ResolveRefError::Empty.into());
        }
        let base_cwd = cwd
            .map(|c| c.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let base_cwd = base_cwd.canonicalize().unwrap_or(base_cwd);

        if let Some((candidate, rel_path)) = self.resolve_existing_path(raw_ref, &base_cwd)? {
            if let Some((fnode, title)) = queries::resolve_ref_by_path(&self.conn, &rel_path)? {
                return Ok((fnode, title, candidate));
            }
            let snapshot = crate::workspace::FileSnapshot::capture(&candidate)?;
            let content = snapshot
                .content()
                .ok_or_else(|| ResolveRefError::Invalid(candidate.display().to_string()))?;
            match MdocHead::load_bytes(&candidate, content) {
                Ok(head) => return Ok((head.fnode, head.title, candidate)),
                Err(_) => {
                    let identity = MdocIdentity::from_bytes(content);
                    if let Some((fnode, title)) = identity.complete() {
                        return Ok((fnode.to_string(), title.to_string(), candidate));
                    }
                    return Err(ResolveRefError::Invalid(candidate.display().to_string()).into());
                }
            }
        }

        let untrusted_rows = queries::resolve_fnode_ref(&self.conn, raw_ref)?
            .ok_or_else(|| ResolveRefError::NotFound(raw_ref.to_string()))?;
        let mut rows = Vec::new();
        for (fnode, title, rel_path) in untrusted_rows {
            if let Some(path) = self.valid_cached_path(&rel_path)? {
                rows.push((fnode, title, rel_path, path));
            }
        }
        if rows.is_empty() {
            return Err(ResolveRefError::NotFound(raw_ref.to_string()).into());
        }

        let query_lc = raw_ref.to_lowercase();
        let exact: Vec<_> = rows
            .iter()
            .filter(|(f, _, _, _)| f.to_lowercase() == query_lc)
            .collect();

        let chosen = if !exact.is_empty() {
            if exact.len() == 1 {
                exact[0]
            } else {
                return Err(ResolveRefError::Ambiguous {
                    reference: raw_ref.to_string(),
                    matches: format_ref_preview(&exact),
                }
                .into());
            }
        } else if rows.len() == 1 {
            &rows[0]
        } else {
            return Err(ResolveRefError::Ambiguous {
                reference: raw_ref.to_string(),
                matches: format_ref_preview(&rows.iter().collect::<Vec<_>>()),
            }
            .into());
        };
        Ok((chosen.0.clone(), chosen.1.clone(), chosen.3.clone()))
    }

    /// Resolve a browser start reference, additionally accepting a unique exact title.
    pub fn resolve_start_ref(
        &self,
        raw_ref: &str,
        cwd: Option<&Path>,
    ) -> Result<(String, String, PathBuf)> {
        let ref_error = match self.resolve_ref(raw_ref, cwd) {
            Ok(resolved) => return Ok(resolved),
            Err(error) => error,
        };
        if !matches!(
            ref_error.downcast_ref::<ResolveRefError>(),
            Some(ResolveRefError::NotFound(_))
        ) {
            return Err(ref_error);
        }
        self.with_current_database(|connection| {
            let raw_ref = raw_ref.trim();
            if raw_ref.is_empty() {
                return Err(ref_error);
            }

            let mut rows = Vec::new();
            for (fnode, title, rel_path) in queries::exact_title_rows(connection, raw_ref)? {
                if let Some(path) = self.valid_cached_path(&rel_path)? {
                    rows.push((fnode, title, rel_path, path));
                }
            }
            match rows.as_slice() {
                [(fnode, title, _, path)] => Ok((fnode.clone(), title.clone(), path.clone())),
                [] => Err(ref_error),
                _ => Err(ResolveRefError::Ambiguous {
                    reference: raw_ref.to_string(),
                    matches: format_ref_preview(&rows.iter().collect::<Vec<_>>()),
                }
                .into()),
            }
        })
    }

    /// Like `resolve_ref` but returns only the path (also accepts refs that aren't indexed).
    pub fn resolve_edit_target_path(&self, raw_ref: &str, cwd: Option<&Path>) -> Result<PathBuf> {
        self.with_current_database(|_| {
            let raw_ref = raw_ref.trim();
            if raw_ref.is_empty() {
                return Err(ResolveRefError::Empty.into());
            }
            let base_cwd = cwd
                .map(|c| c.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let base_cwd = base_cwd.canonicalize().unwrap_or(base_cwd);
            if let Some((candidate, _)) = self.resolve_existing_path(raw_ref, &base_cwd)? {
                return Ok(candidate);
            }
            let (_, _, path) = self.resolve_ref_inner(raw_ref, Some(&base_cwd))?;
            Ok(path)
        })
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn valid_cached_path(&self, rel_path: &str) -> Result<Option<PathBuf>> {
        refresh::current_cached_mdoc_path(&self.root, rel_path)
    }

    /// If `raw_ref` looks like a path, try to resolve it to an existing file.
    /// Returns `(abs_path, rel_path)` on success.
    fn resolve_existing_path(
        &self,
        raw_ref: &str,
        cwd: &Path,
    ) -> Result<Option<(PathBuf, String)>> {
        let mut raw_path = PathBuf::from(raw_ref);
        if !looks_like_path_ref(raw_ref) && raw_path.extension().is_some() {
            return Ok(None);
        }
        if raw_path.extension().is_none() {
            raw_path.set_extension("mdoc");
        }
        let candidates: Vec<PathBuf> = if raw_path.is_absolute() {
            vec![raw_path]
        } else {
            vec![cwd.join(&raw_path), self.root.join(&raw_path)]
        };
        for candidate in candidates {
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    let resolved = crate::workspace::resolve_mdoc_path(&self.root, &candidate)?;
                    let meta = std::fs::symlink_metadata(&resolved)?;
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        bail!("mdoc path is not a regular file: {}", candidate.display());
                    }
                    let rel_path = crate::workspace::to_indexed_rel_path(&self.root, &resolved)?;
                    return Ok(Some((resolved, rel_path)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting {}", candidate.display()))
                }
            }
        }
        if raw_ref.ends_with(".mdoc") {
            return Err(ResolveRefError::NotFound(raw_ref.to_string()).into());
        }
        Ok(None)
    }
}

impl MutationSession<'_, '_> {
    pub(crate) fn store_mut(&mut self) -> &mut WorkspaceStore {
        self.store
    }

    pub(crate) fn mark_dirty(&mut self) -> Result<()> {
        if !self.dirty {
            self.store.validate_mutation_lock(self.lock)?;
            self.store.set_index_dirty(true)?;
            self.dirty = true;
        }
        Ok(())
    }

    pub(crate) fn create_node(
        &mut self,
        node: &MdocNode,
    ) -> Result<crate::workspace::AppliedWrite> {
        self.store.validate_mutation_lock(self.lock)?;
        let path = self.store.validate_node_path(node)?;
        let payload = node.render()?;
        if let Some(parent) = path.parent() {
            crate::workspace::ensure_regular_directory_tree(&self.store.root, parent)
                .with_context(|| format!("creating parent dirs for {}", path.display()))?;
        }
        self.store.validate_mutation_lock(self.lock)?;
        let path = self.store.validate_node_path(node)?;
        self.mark_dirty()?;
        self.store.write_and_index_node(
            self.lock,
            &path,
            payload.as_bytes(),
            &crate::workspace::FileSnapshot::Missing,
        )
    }

    pub(crate) fn replace_node(
        &mut self,
        node: &MdocNode,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<()> {
        self.store.validate_mutation_lock(self.lock)?;
        let path = self.store.validate_node_path(node)?;
        let payload = node.render()?;
        self.mark_dirty()?;
        self.store
            .write_and_index_node(self.lock, &path, payload.as_bytes(), snapshot)?;
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        self.store.validate_mutation_lock(self.lock)?;
        if self.dirty {
            self.store.set_index_dirty(false)?;
            self.dirty = false;
        }
        Ok(())
    }

    fn abort(&mut self) -> Result<()> {
        if self.dirty {
            self.store.validate_mutation_lock(self.lock)?;
            self.store.recover_index()?;
            self.dirty = false;
        }
        Ok(())
    }
}

fn looks_like_path_ref(raw_ref: &str) -> bool {
    raw_ref.contains('/') || raw_ref.ends_with(".mdoc") || raw_ref.starts_with('.')
}

fn format_ref_preview(rows: &[&(String, String, String, PathBuf)]) -> String {
    rows.iter()
        .map(|(f, _, p, _)| format!("{}:{}", short_fnode(f), p))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod mutation_boundary_tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".mdc")).unwrap();
        dir
    }

    fn write_node(node: &MdocNode) {
        std::fs::write(&node.path, node.render().unwrap()).unwrap();
    }

    #[test]
    fn discovery_indexes_a_stale_candidate_recreated_before_apply() {
        let workspace = workspace();
        let root = workspace.path();
        let path = root.join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "recreated-node".to_string();
        write_node(&node);
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        std::fs::remove_file(&path).unwrap();
        node.title = "Recreated".to_string();
        let replacement = node.render().unwrap();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::DiscoveryBeforeApply,
            move || std::fs::write(path, replacement).unwrap(),
        );

        cache.discover_workspace_changes().unwrap();

        assert_eq!(cache.node_summary(&node.fnode).unwrap().title, "Recreated");
    }

    #[test]
    fn post_index_external_edit_is_preserved_and_reindexed() {
        let workspace = workspace();
        let root = workspace.path();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = root.join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Requested");
        node.fnode = "requested-node".to_string();
        let mut external = MdocNode::new_at_path(&path, "External");
        external.fnode = "external-node".to_string();
        let external_content = external.render().unwrap();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::IndexAfterNodeUpsert,
            move || std::fs::write(path, external_content).unwrap(),
        );

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error
            .downcast_ref::<crate::workspace::PersistenceRecoveryError>()
            .is_some_and(|error| error.has_file_conflict()));
        assert_eq!(
            MdocNode::load(&external.path).unwrap().fnode,
            external.fnode
        );
        assert_eq!(
            cache.node_summary(&external.fnode).unwrap().title,
            "External"
        );
        assert!(cache.node_summary(&node.fnode).is_err());
    }

    #[test]
    fn create_rejects_a_mutation_lock_from_another_cache() {
        let first = workspace();
        let second = workspace();
        let mut cache = IndCache::open(first.path().to_path_buf()).unwrap();
        let other_cache = IndCache::open(second.path().to_path_buf()).unwrap();
        let other_lock = other_cache.acquire_mutation_lock().unwrap();
        let path = first.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&other_lock, &node).unwrap_err();

        assert!(error.to_string().contains("does not match cache root"));
        assert!(!path.exists());
    }

    #[test]
    fn create_rejects_a_replaced_index_database_generation() {
        let workspace = workspace();
        let root = workspace.path();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let db_path = root.join(".mdc/index.db");
        std::fs::rename(&db_path, root.join("detached-index.db")).unwrap();
        std::fs::write(&db_path, []).unwrap();
        let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(root).unwrap();
        let path = root.join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("index database changed"));
        assert!(!path.exists());
    }

    #[test]
    fn create_rolls_back_after_index_replacement_during_persistence() {
        let workspace = workspace();
        let root = workspace.path();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let db_path = root.join(".mdc/index.db");
        let detached = root.join("detached-index.db");
        crate::workspace::set_test_hook(crate::workspace::TestHookPoint::WriteAfterPersistence, {
            let db_path = db_path.clone();
            move || {
                std::fs::rename(&db_path, detached).unwrap();
                std::fs::write(&db_path, []).unwrap();
            }
        });
        let path = root.join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("index database changed"));
        assert!(!path.exists());
    }

    #[test]
    fn lock_replacement_after_index_commit_preserves_file_index_agreement() {
        let workspace = workspace();
        let root = workspace.path();
        let mut cache = IndCache::open(root.to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let lock_path = root.join(".mdc/mutation.lock");
        let displaced_lock = root.join("displaced-mutation.lock");
        crate::workspace::set_test_hook(crate::workspace::TestHookPoint::IndexAfterNodeUpsert, {
            let lock_path = lock_path.clone();
            move || {
                std::fs::rename(&lock_path, displaced_lock).unwrap();
                std::fs::write(&lock_path, []).unwrap();
            }
        });
        let path = root.join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "committed-node".to_string();

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("uncertain lock"));
        assert_eq!(MdocNode::load(&path).unwrap().fnode, "committed-node");
        assert_eq!(cache.exact_fnode_rows("committed-node").unwrap().len(), 1);
    }

    #[test]
    fn open_rejects_a_restored_guard_around_a_displaced_sqlite_connection() {
        let workspace = workspace();
        let root = workspace.path();
        drop(IndCache::open(root.to_path_buf()).unwrap());
        let db_path = root.join(".mdc/index.db");
        let guarded = root.join("guarded-index.db");
        let connected = root.join("connected-index.db");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::IndexBeforeConnectionOpen,
            {
                let db_path = db_path.clone();
                let guarded = guarded.clone();
                let connected = connected.clone();
                move || {
                    std::fs::rename(&db_path, &guarded).unwrap();
                    std::fs::write(&db_path, []).unwrap();
                    crate::workspace::set_test_hook(
                        crate::workspace::TestHookPoint::IndexAfterConnectionOpen,
                        move || {
                            std::fs::rename(&db_path, connected).unwrap();
                            std::fs::rename(guarded, &db_path).unwrap();
                        },
                    );
                }
            },
        );

        let error = IndCache::open(root.to_path_buf())
            .err()
            .expect("displaced SQLite connection must be rejected");

        assert!(error.to_string().contains("displaced index database"));
    }

    #[test]
    fn open_revalidates_the_mutation_lock_after_sqlite_open() {
        let workspace = workspace();
        let root = workspace.path();
        let lock_path = root.join(".mdc/mutation.lock");
        let displaced = root.join("displaced-mutation.lock");
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::IndexAfterConnectionOpen,
            {
                let lock_path = lock_path.clone();
                move || {
                    std::fs::rename(&lock_path, displaced).unwrap();
                    std::fs::write(&lock_path, []).unwrap();
                }
            },
        );

        let error = IndCache::open(root.to_path_buf())
            .err()
            .expect("replaced mutation lock must invalidate cache open");

        assert!(format!("{error:#}").contains("workspace mutation lock"));
    }

    #[test]
    fn create_rejects_a_node_from_another_workspace_before_writing() {
        let first = workspace();
        let second = workspace();
        let mut cache = IndCache::open(first.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = second.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("outside workspace"));
        assert!(!path.exists());
    }

    #[test]
    fn create_builds_parents_and_indexes_the_validated_path() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("notes/nested/node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();

        cache.create_node(&mutation_lock, &node).unwrap();

        assert!(path.is_file());
        assert_eq!(cache.exact_fnode_rows("created-node").unwrap().len(), 1);
        assert!(!cache.index_is_dirty().unwrap());
    }

    #[test]
    fn mutation_session_owns_the_dirty_marker_until_commit() {
        let workspace = workspace();
        let mut store = WorkspaceStore::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = store.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");
        let mut mutation = store.mutation_session(&mutation_lock).unwrap();

        let _receipt = mutation.create_node(&node).unwrap();
        assert!(mutation.store_mut().index_is_dirty().unwrap());
        mutation.commit().unwrap();

        assert!(!store.index_is_dirty().unwrap());
        assert_eq!(store.node_summary(&node.fnode).unwrap().title, "Node");
    }

    #[test]
    fn create_rolls_back_file_and_recovers_index_after_index_failure() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_created_node
                 BEFORE INSERT ON mdocs
                 WHEN NEW.fnode = 'created-node'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected index failure');
                 END;",
            )
            .unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("created.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(error.to_string().contains("injected index failure"));
        assert!(!path.exists());
        assert!(cache.exact_fnode_rows("created-node").unwrap().is_empty());
        assert!(!cache.index_is_dirty().unwrap());
    }

    #[test]
    fn create_recovery_preserves_typed_rollback_conflict() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        cache
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_created_node
                 BEFORE INSERT ON mdocs
                 WHEN NEW.fnode = 'created-node'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected index failure');
                 END;",
            )
            .unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("created.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Node");
        node.fnode = "created-node".to_string();
        let edited_path = path.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::RollbackAfterInitialVerification,
            move || {
                std::fs::write(
                    edited_path,
                    "@fnode: external-node\n@title: External edit\n",
                )
                .unwrap();
            },
        );

        let error = cache.create_node(&mutation_lock, &node).unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(crate::workspace::error_has_infrastructure_failure(&error));
        assert_eq!(MdocNode::load(&path).unwrap().title, "External edit");
        assert_eq!(
            cache.node_summary("external-node").unwrap().title,
            "External edit"
        );
    }

    #[test]
    fn replace_rejects_an_outside_path_before_writing() {
        let workspace = workspace();
        let outside = tempfile::TempDir::new().unwrap();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = outside.path().join("node.mdoc");
        let node = MdocNode::new_at_path(&path, "Node");

        let error = cache
            .replace_node(
                &mutation_lock,
                &node,
                &crate::workspace::FileSnapshot::Missing,
            )
            .unwrap_err();

        assert!(error.to_string().contains("outside workspace"));
        assert!(!path.exists());
    }

    #[test]
    fn stale_replace_preserves_external_edit() {
        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("node.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "stale-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();

        let mut desired = node.clone();
        desired.title = "Desired".to_string();
        let mut external = node.clone();
        external.title = "External edit".to_string();
        write_node(&external);

        let error = cache
            .replace_node(&mutation_lock, &desired, &snapshot)
            .unwrap_err();

        assert!(error
            .downcast_ref::<crate::workspace::FileConflict>()
            .is_some());
        assert_eq!(MdocNode::load(&path).unwrap().title, "External edit");
    }

    #[cfg(unix)]
    #[test]
    fn replace_rejects_read_only_files() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("readonly.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "readonly-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();
        node.title = "Changed".to_string();

        let error = cache
            .replace_node(&mutation_lock, &node, &snapshot)
            .unwrap_err();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(error.to_string().contains("read-only"));
        assert_eq!(MdocNode::load(&path).unwrap().title, "Original");
    }

    #[cfg(unix)]
    #[test]
    fn replace_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = workspace();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let path = workspace.path().join("metadata.mdoc");
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.fnode = "metadata-node".to_string();
        let _receipt = cache.create_node(&mutation_lock, &node).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let snapshot = crate::workspace::FileSnapshot::capture(&path).unwrap();
        node.title = "Changed".to_string();

        cache
            .replace_node(&mutation_lock, &node, &snapshot)
            .unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(MdocNode::load(&path).unwrap().title, "Changed");
    }

    #[cfg(unix)]
    #[test]
    fn create_rejects_final_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let workspace = workspace();
        let outside = tempfile::TempDir::new().unwrap();
        let outside_path = outside.path().join("victim.mdoc");
        std::fs::write(&outside_path, "victim").unwrap();
        let path = workspace.path().join("link.mdoc");
        symlink(&outside_path, &path).unwrap();
        let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        let mutation_lock = cache.acquire_mutation_lock().unwrap();
        let node = MdocNode::new_at_path(&path, "Node");

        assert!(cache.create_node(&mutation_lock, &node).is_err());
        assert_eq!(std::fs::read_to_string(outside_path).unwrap(), "victim");
        assert!(std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn ancestor_replacement_cannot_redirect_indexed_create_or_replace() {
        use std::os::unix::fs::symlink;

        for replacing in [false, true] {
            let workspace = workspace();
            let outside = tempfile::TempDir::new().unwrap();
            let ancestor = workspace.path().join("ancestor");
            let parent = ancestor.join("parent");
            std::fs::create_dir_all(&parent).unwrap();
            std::fs::create_dir(outside.path().join("parent")).unwrap();
            let path = parent.join("node.mdoc");
            let outside_path = outside.path().join("parent/node.mdoc");
            let displaced = workspace.path().join("displaced");
            let mut node = MdocNode::new_at_path(&path, "After");
            node.fnode = "ancestor-race-node".to_string();
            let snapshot = if replacing {
                let mut before = node.clone();
                before.title = "Before".to_string();
                write_node(&before);
                std::fs::write(&outside_path, b"outside victim").unwrap();
                crate::workspace::FileSnapshot::capture(&path).unwrap()
            } else {
                crate::workspace::FileSnapshot::Missing
            };
            let mut cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
            let mutation_lock = cache.acquire_mutation_lock().unwrap();
            let hook_ancestor = ancestor.clone();
            let hook_outside = outside.path().to_path_buf();
            let hook_displaced = displaced.clone();
            crate::workspace::set_test_hook(
                crate::workspace::TestHookPoint::WriteBeforeDirectoryBinding,
                move || {
                    std::fs::rename(&hook_ancestor, &hook_displaced).unwrap();
                    symlink(&hook_outside, &hook_ancestor).unwrap();
                },
            );

            let error = if replacing {
                cache
                    .replace_node(&mutation_lock, &node, &snapshot)
                    .unwrap_err()
            } else {
                cache.create_node(&mutation_lock, &node).unwrap_err()
            };

            assert!(crate::workspace::error_has_file_conflict(&error));
            if replacing {
                assert_eq!(std::fs::read(&outside_path).unwrap(), b"outside victim");
                assert_eq!(
                    MdocNode::load(&displaced.join("parent/node.mdoc"))
                        .unwrap()
                        .title,
                    "Before"
                );
            } else {
                assert!(!outside_path.exists());
                assert!(!displaced.join("parent/node.mdoc").exists());
            }
        }
    }

    #[test]
    fn cache_rejects_a_replaced_control_directory() {
        let workspace = workspace();
        let cache = IndCache::open(workspace.path().to_path_buf()).unwrap();
        std::fs::rename(
            workspace.path().join(".mdc"),
            workspace.path().join("old-mdc"),
        )
        .unwrap();
        std::fs::create_dir(workspace.path().join(".mdc")).unwrap();
        std::fs::rename(
            workspace.path().join("old-mdc/index.db"),
            workspace.path().join(".mdc/index.db"),
        )
        .unwrap();

        let read_error = cache.count().unwrap_err();
        assert!(read_error.to_string().contains("control directory changed"));

        let error = match cache.acquire_mutation_lock() {
            Err(error) => error,
            Ok(_) => panic!("expected replaced control directory to be rejected"),
        };

        assert!(error.to_string().contains("does not match the cache"));
    }
}
