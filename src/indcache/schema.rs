use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

const SCHEMA_VERSION: i32 = 16;

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS mdocs (
    path        TEXT    PRIMARY KEY,
    fnode       TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    title_lc    TEXT    NOT NULL,
    topo_depth  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc);
CREATE INDEX IF NOT EXISTS idx_mdocs_fnode    ON mdocs(fnode);

CREATE TABLE IF NOT EXISTS mdoc_files (
    path     TEXT    PRIMARY KEY,
    mtime_ns INTEGER NOT NULL,
    size     INTEGER NOT NULL
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

CREATE VIEW IF NOT EXISTS mdoc_valid_edges AS
SELECT e.src_path, e.src_fnode, e.dst_fnode, e.ord
FROM mdoc_edges e
WHERE NOT EXISTS (
    SELECT 1 FROM mdoc_issues i
    WHERE i.path = e.src_path
      AND i.kind IN ('invalid', 'duplicate')
);

CREATE TABLE IF NOT EXISTS mdoc_index_state (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    bootstrapped         INTEGER NOT NULL DEFAULT 0,
    graph_epoch          INTEGER NOT NULL DEFAULT 0,
    weak_component_dirty INTEGER NOT NULL DEFAULT 1,
    index_digest         TEXT    NOT NULL DEFAULT ''
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
    component_size INTEGER NOT NULL DEFAULT 1
);
";

const RESET_SQL: &str = "
DROP VIEW IF EXISTS mdoc_valid_edges;
DROP TABLE IF EXISTS mdoc_weak_component;
DROP TABLE IF EXISTS mdoc_scc_result;
DROP TABLE IF EXISTS mdoc_in_degree;
DROP TABLE IF EXISTS mdoc_index_state;
DROP TABLE IF EXISTS mdoc_issues;
DROP TABLE IF EXISTS mdoc_edges;
DROP TABLE IF EXISTS mdoc_dirs;
DROP TABLE IF EXISTS mdoc_files;
DROP TABLE IF EXISTS mdocs;
";

/// Open the database at `path` with WAL mode and apply the schema.
pub fn open_db(path: &Path) -> Result<Connection> {
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
        Ok(meta) => reject_multiply_linked_file(&path, &meta)?,
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

fn reject_multiply_linked_file(path: &Path, meta: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if meta.nlink() > 1 {
        bail!(
            "refusing to open multiply linked index database {}",
            path.display()
        );
    }
    Ok(())
}

fn open_db_once(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    checked_user_version(&conn)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    apply_schema(&mut conn)?;
    Ok(conn)
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

/// Rebuild old derived indexes instead of migrating derived rows in place.
fn apply_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let user_version = checked_user_version(&tx)?;

    if user_version < SCHEMA_VERSION {
        tx.execute_batch(RESET_SQL)?;
        tx.execute_batch(CREATE_SQL)?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    } else {
        tx.execute_batch(CREATE_SQL)?;
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_fresh_db() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();
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
    fn valid_edge_view_filters_only_blocking_source_issues() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord) VALUES
                 ('valid.mdoc', 'valid', 'target', 0),
                 ('missing.mdoc', 'missing', 'target', 0),
                 ('invalid.mdoc', 'invalid', 'target', 0),
                 ('duplicate.mdoc', 'duplicate', 'target', 0);
             INSERT INTO mdoc_issues (path, kind, ref_fnode, error) VALUES
                 ('missing.mdoc', 'missing', 'absent', 'missing target'),
                 ('invalid.mdoc', 'invalid', 'invalid', 'invalid source'),
                 ('duplicate.mdoc', 'duplicate', 'duplicate', 'duplicate source');",
        )
        .unwrap();

        let rows: Vec<(String, String)> = conn
            .prepare("SELECT src_fnode, dst_fnode FROM mdoc_valid_edges ORDER BY src_fnode")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(
            rows,
            vec![
                ("missing".to_string(), "target".to_string()),
                ("valid".to_string(), "target".to_string()),
            ]
        );
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        open_db(&path).unwrap();
        open_db(&path).unwrap(); // second open should not fail
    }

    #[test]
    fn hard_linked_database_is_rejected_without_mutating_alias() {
        let dir = TempDir::new().unwrap();
        let external = dir.path().join("external.db");
        let index = dir.path().join("index.db");
        std::fs::write(&external, b"external database bytes").unwrap();
        std::fs::hard_link(&external, &index).unwrap();
        let before = std::fs::read(&external).unwrap();

        let error = open_db(&index).unwrap_err();

        assert!(error.to_string().contains("multiply linked"));
        assert_eq!(std::fs::read(&external).unwrap(), before);
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
    fn old_cache_is_rebuilt_with_current_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
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

        let conn = open_db(&path).unwrap();
        let has_metadata: bool = conn
            .query_row(
                "SELECT COUNT(*) = 2 FROM pragma_table_info('mdoc_files')
                 WHERE name IN ('mtime_ns', 'size')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_metadata);
    }

    #[test]
    fn rebuilding_an_old_cache_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let mut conn = open_db(&path).unwrap();
        // Old derived rows are discarded rather than migrated in place.
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        conn.execute_batch("INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord) VALUES ('a.mdoc', 'fa', 'fb', 0)").unwrap();
        apply_schema(&mut conn).unwrap();
        let edges: i32 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edges, 0);
        // Applying the current schema again must not discard new data or fail.
        apply_schema(&mut conn).unwrap();
    }

    #[test]
    fn schema_fifteen_is_rebuilt_with_an_index_digest() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE mdoc_index_state DROP COLUMN index_digest;
             PRAGMA user_version = 15;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();
        let digest: String = conn
            .query_row(
                "SELECT index_digest FROM mdoc_index_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(digest.is_empty());
    }
}
