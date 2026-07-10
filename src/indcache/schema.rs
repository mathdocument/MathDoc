use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

const SCHEMA_VERSION: i32 = 13;

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS mdocs (
    path        TEXT    PRIMARY KEY,
    fnode       TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    title_lc    TEXT    NOT NULL,
    mtime_sec   INTEGER NOT NULL,
    mtime_ns    INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    topo_depth  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc);
CREATE INDEX IF NOT EXISTS idx_mdocs_fnode    ON mdocs(fnode);

CREATE TABLE IF NOT EXISTS mdoc_files (
    path      TEXT PRIMARY KEY,
    mtime_sec INTEGER NOT NULL,
    mtime_ns  INTEGER NOT NULL,
    size      INTEGER NOT NULL,
    digest    BLOB    NOT NULL DEFAULT X''
);

CREATE TABLE IF NOT EXISTS mdoc_dirs (
    path     TEXT PRIMARY KEY,
    mtime_ns INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mdoc_edges (
    src_path  TEXT    NOT NULL,
    src_fnode TEXT    NOT NULL,
    dst_fnode TEXT    NOT NULL,
    ord       INTEGER NOT NULL,
    PRIMARY KEY (src_path, ord)
);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_fnode ON mdoc_edges(src_fnode);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_fnode ON mdoc_edges(dst_fnode);

CREATE TABLE IF NOT EXISTS mdoc_issues (
    path      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    ref_fnode TEXT NOT NULL,
    error     TEXT NOT NULL,
    PRIMARY KEY (path, kind, ref_fnode)
);
CREATE INDEX IF NOT EXISTS idx_mdoc_issues_kind      ON mdoc_issues(kind);
CREATE INDEX IF NOT EXISTS idx_mdoc_issues_ref_fnode ON mdoc_issues(ref_fnode);

CREATE TABLE IF NOT EXISTS mdoc_index_state (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    bootstrapped            INTEGER NOT NULL DEFAULT 0,
    graph_epoch             INTEGER NOT NULL DEFAULT 0,
    weak_component_dirty    INTEGER NOT NULL DEFAULT 1,
    topo_depth_backfilled   INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO mdoc_index_state (id, bootstrapped) VALUES (1, 0);

CREATE TABLE IF NOT EXISTS mdoc_in_degree (
    fnode     TEXT    PRIMARY KEY,
    in_degree INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS mdoc_scc_result (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    graph_epoch INTEGER NOT NULL DEFAULT -1,
    cycles_json TEXT    NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS mdoc_weak_component (
    fnode          TEXT    PRIMARY KEY,
    component_id   TEXT    NOT NULL DEFAULT '',
    component_size INTEGER NOT NULL DEFAULT 1
);
";

const BACKFILL_IN_DEGREE_SQL: &str = "
DELETE FROM mdoc_in_degree;
INSERT INTO mdoc_in_degree (fnode, in_degree)
    SELECT dst_fnode, COUNT(*)
    FROM mdoc_edges
    WHERE NOT EXISTS (
        SELECT 1 FROM mdoc_issues
        WHERE mdoc_issues.path  = mdoc_edges.src_path
          AND mdoc_issues.kind IN ('invalid', 'duplicate')
    )
    GROUP BY dst_fnode;
";

const MIGRATE_MDOCS_PRIMARY_KEY_SQL: &str = "
ALTER TABLE mdocs RENAME TO mdocs_legacy;
CREATE TABLE mdocs (
    path        TEXT    PRIMARY KEY,
    fnode       TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    title_lc    TEXT    NOT NULL,
    mtime_sec   INTEGER NOT NULL,
    mtime_ns    INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    topo_depth  INTEGER NOT NULL DEFAULT 0
);
INSERT INTO mdocs (path, fnode, title, title_lc, mtime_sec, mtime_ns, size, topo_depth)
    SELECT path, fnode, title, title_lc, mtime_sec, mtime_ns, size, topo_depth
    FROM mdocs_legacy;
DROP TABLE mdocs_legacy;
CREATE INDEX idx_mdocs_title_lc ON mdocs(title_lc);
CREATE INDEX idx_mdocs_fnode    ON mdocs(fnode);
";

/// Open the database at `path` with WAL mode and apply the schema.
/// Returns `(connection, needs_topo_backfill)`.  When `needs_topo_backfill` is
/// true the caller must run `backfill_all_topo_depths` before serving reads,
/// because the `topo_depth` column was just added with all-zero defaults.
pub fn open_db(path: &Path) -> Result<(Connection, bool)> {
    let file_name = path.file_name().ok_or_else(|| {
        anyhow::anyhow!("index database path has no file name: {}", path.display())
    })?;
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .with_context(|| format!("canonicalizing index directory for {}", path.display()))?;
    let path = parent.join(file_name);

    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!(
                "refusing to open symlinked index database {}",
                path.display()
            )
        }
        Ok(meta) if !meta.is_file() => {
            bail!("index database is not a regular file: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match open_db_once(&path) {
            Err(error) if is_database_busy(&error) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn open_db_once(path: &Path) -> Result<(Connection, bool)> {
    let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    checked_user_version(&conn)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    let needs_topo_backfill = apply_schema(&mut conn)?;
    Ok((conn, needs_topo_backfill))
}

fn checked_user_version(conn: &Connection) -> Result<i32> {
    let user_version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if user_version > SCHEMA_VERSION {
        bail!(
            "index schema version {user_version} is newer than supported version {SCHEMA_VERSION}"
        );
    }
    Ok(user_version)
}

fn is_database_busy(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<rusqlite::Error>()
        .and_then(rusqlite::Error::sqlite_error_code)
        .is_some_and(|code| {
            matches!(
                code,
                rusqlite::ffi::ErrorCode::DatabaseBusy | rusqlite::ffi::ErrorCode::DatabaseLocked
            )
        })
}

/// Returns `true` when `topo_depth` needs to be backfilled before the first read.
///
/// The flag is checked on *every* open (outside the version guard) so that a
/// crash between the version bump and the actual backfill is automatically
/// recovered on the next startup.
fn apply_schema(conn: &mut Connection) -> Result<bool> {
    let tx = conn.transaction()?;
    let user_version = checked_user_version(&tx)?;
    tx.execute_batch(CREATE_SQL)?;

    if user_version < SCHEMA_VERSION {
        // Add mtime_ns to mdocs if missing (v4→v5 migration).
        let has_mtime_ns: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdocs') WHERE name = 'mtime_ns'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_mtime_ns {
            tx.execute_batch("ALTER TABLE mdocs ADD COLUMN mtime_ns INTEGER NOT NULL DEFAULT 0;")?;
        }

        let has_digest: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdoc_files') WHERE name = 'digest'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_digest {
            tx.execute_batch(
                "ALTER TABLE mdoc_files ADD COLUMN digest BLOB NOT NULL DEFAULT X'';",
            )?;
        }

        // Add topo_depth to mdocs if missing (v5→v6 migration).
        let has_topo_depth: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdocs') WHERE name = 'topo_depth'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_topo_depth {
            tx.execute_batch(
                "ALTER TABLE mdocs ADD COLUMN topo_depth INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        // Add component_id to mdoc_weak_component if missing (v6→v7 migration).
        let has_component_id: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdoc_weak_component') WHERE name = 'component_id'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_component_id {
            tx.execute_batch(
                "ALTER TABLE mdoc_weak_component ADD COLUMN component_id TEXT NOT NULL DEFAULT '';
                 UPDATE mdoc_weak_component SET component_id = fnode WHERE component_id = '';
                 UPDATE mdoc_index_state SET weak_component_dirty = 1 WHERE id = 1;",
            )?;
        }

        // Add topo_depth_backfilled flag to mdoc_index_state if missing (v7→v8 migration).
        // Default 0 means "not yet backfilled"; IndCache::open will run the backfill and
        // set it to 1 in the same transaction, making recovery crash-safe.
        let has_topo_flag: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdoc_index_state') WHERE name = 'topo_depth_backfilled'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_topo_flag {
            tx.execute_batch(
                "ALTER TABLE mdoc_index_state ADD COLUMN topo_depth_backfilled INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        if mdocs_needs_primary_key_migration(&tx)? {
            tx.execute_batch(MIGRATE_MDOCS_PRIMARY_KEY_SQL)?;
        }

        // v8 incremental unions could persist inconsistent component sizes.
        // Force the established lazy full recomputation on first graph read.
        tx.execute(
            "UPDATE mdoc_index_state
             SET weak_component_dirty = 1,
                 topo_depth_backfilled = 0,
                 bootstrapped = 0
             WHERE id = 1",
            [],
        )?;
        tx.execute_batch(BACKFILL_IN_DEGREE_SQL)?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }

    // Check the persistent flag on every open — independent of user_version so that
    // a crash between the version bump and the backfill is recovered automatically.
    let needs_topo_backfill: bool = tx
        .query_row(
            "SELECT topo_depth_backfilled FROM mdoc_index_state WHERE id = 1",
            [],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 0)
        .unwrap_or(false);

    tx.commit()?;
    Ok(needs_topo_backfill)
}

fn mdocs_needs_primary_key_migration(conn: &Connection) -> Result<bool> {
    let path_is_only_primary_key: bool = conn.query_row(
        "SELECT COUNT(*) = 1
                AND MAX(CASE WHEN name = 'path' THEN pk ELSE 0 END) = 1
         FROM pragma_table_info('mdocs')
         WHERE pk > 0",
        [],
        |r| r.get(0),
    )?;
    let fnode_is_unique: bool = conn.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM pragma_index_list('mdocs') AS indexes
             WHERE indexes.[unique] = 1
               AND (SELECT COUNT(*) FROM pragma_index_info(indexes.name)) = 1
               AND (SELECT name FROM pragma_index_info(indexes.name) LIMIT 1) = 'fnode'
         )",
        [],
        |r| r.get(0),
    )?;
    Ok(!path_is_only_primary_key || fnode_is_unique)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_fresh_db() {
        let dir = TempDir::new().unwrap();
        // Fresh DB: topo_depth_backfilled defaults to 0, so backfill is requested
        // (no-op on empty DB, but the flag machinery should still trigger).
        let (conn, needs_backfill) = open_db(&dir.path().join("index.db")).unwrap();
        assert!(needs_backfill);
        let n: i32 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_index_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        open_db(&path).unwrap();
        open_db(&path).unwrap(); // second open should not fail
    }

    #[test]
    fn future_schema_is_rejected_without_mutation() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let future_version = SCHEMA_VERSION + 1;
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE future_data (value TEXT);
             INSERT INTO future_data VALUES ('preserve me');
             PRAGMA user_version = {future_version};"
        ))
        .unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();

        let error = open_db(&path).unwrap_err();
        assert!(error.to_string().contains("newer than supported"));
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            checked_user_version(&conn).unwrap_err().to_string(),
            error.to_string()
        );
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        let value: String = conn
            .query_row("SELECT value FROM future_data", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "preserve me");
        let managed_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'mdocs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(managed_tables, 0);
    }

    #[test]
    fn version_ten_cache_gains_content_digest() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let (conn, _) = open_db(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE mdoc_files RENAME TO mdoc_files_new;
             CREATE TABLE mdoc_files (
                 path TEXT PRIMARY KEY,
                 mtime_sec INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 size INTEGER NOT NULL
             );
             DROP TABLE mdoc_files_new;
             PRAGMA user_version = 10;",
        )
        .unwrap();
        drop(conn);

        let (conn, _) = open_db(&path).unwrap();
        let has_digest: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdoc_files') WHERE name = 'digest'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count == 1)
            .unwrap();
        assert!(has_digest);
    }

    #[test]
    fn backfill_migration_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let (mut conn, _) = open_db(&path).unwrap();
        // Simulate an old database by resetting user_version, then re-apply
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        conn.execute_batch("INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord) VALUES ('a.mdoc', 'fa', 'fb', 0)").unwrap();
        apply_schema(&mut conn).unwrap();
        let degree: i32 = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = 'fb'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(degree, 1);
        // Apply again — must not error (idempotent)
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        apply_schema(&mut conn).unwrap();
    }
}
