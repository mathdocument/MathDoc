use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

const SCHEMA_VERSION: i32 = 19;
const FIRST_INCREMENTAL_SCHEMA_VERSION: i32 = 17;

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
    path        TEXT    PRIMARY KEY,
    mtime_ns    INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    lean_status INTEGER NOT NULL DEFAULT 0 CHECK (lean_status BETWEEN 0 AND 2),
    rocq_status INTEGER NOT NULL DEFAULT 0 CHECK (rocq_status BETWEEN 0 AND 2)
);
CREATE INDEX IF NOT EXISTS idx_mdoc_files_lean_verified
    ON mdoc_files(lean_status) WHERE lean_status = 2;
CREATE INDEX IF NOT EXISTS idx_mdoc_files_rocq_verified
    ON mdoc_files(rocq_status) WHERE rocq_status = 2;

CREATE TABLE IF NOT EXISTS mdoc_edges (
    src_path  TEXT    NOT NULL,
    src_fnode TEXT    NOT NULL,
    dst_fnode TEXT    NOT NULL,
    ord       INTEGER NOT NULL,
    PRIMARY KEY (src_path, ord)
);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_fnode ON mdoc_edges(src_fnode);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_fnode ON mdoc_edges(dst_fnode);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_dst
    ON mdoc_edges(src_fnode, dst_fnode, src_path);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_src
    ON mdoc_edges(dst_fnode, src_fnode, src_path, ord);

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

CREATE VIEW IF NOT EXISTS mdoc_missing_issues AS
SELECT '<unknown>' AS path,
       'missing' AS kind,
       d.fnode AS ref_fnode,
       'missing dependency target: ' || d.fnode AS error
FROM mdoc_in_degree d
WHERE d.in_degree > 0
  AND NOT EXISTS (
    SELECT 1 FROM mdocs claimant
    WHERE claimant.fnode = d.fnode
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
DROP VIEW IF EXISTS mdoc_missing_issues;
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

const MIGRATE_17_TO_18_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_dst
    ON mdoc_edges(src_fnode, dst_fnode, src_path);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_src
    ON mdoc_edges(dst_fnode, src_fnode, src_path, ord);
";

const MIGRATE_18_TO_19_SQL: &str = "
DELETE FROM mdoc_issues WHERE kind = 'missing';
DELETE FROM mdoc_in_degree;
INSERT INTO mdoc_in_degree (fnode, in_degree)
SELECT dst_fnode, COUNT(*)
FROM mdoc_valid_edges
GROUP BY dst_fnode
HAVING COUNT(*) > 0;
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

/// Migrate compatible schemas in place and rebuild older derived caches.
fn apply_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let mut user_version = checked_user_version(&tx)?;

    if user_version < FIRST_INCREMENTAL_SCHEMA_VERSION {
        tx.execute_batch(RESET_SQL)?;
        tx.execute_batch(CREATE_SQL)?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    } else {
        if user_version == 17 {
            tx.execute_batch(MIGRATE_17_TO_18_SQL)?;
            user_version = 18;
        }
        if user_version == 18 {
            tx.execute_batch(MIGRATE_18_TO_19_SQL)?;
        }
        tx.execute_batch(CREATE_SQL)?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
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
    fn missing_issue_view_uses_valid_edge_degrees_and_any_complete_claimant() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_in_degree (fnode, in_degree) VALUES
                 ('absent-target', 2),
                 ('present-target', 1),
                 ('invalid-target', 1),
                 ('duplicate-target', 1),
                 ('partial-target', 1);
             INSERT INTO mdocs (path, fnode, title, title_lc) VALUES
                 ('present-target.mdoc', 'present-target', 'Present', 'present'),
                 ('invalid-target.mdoc', 'invalid-target', 'Invalid', 'invalid'),
                 ('duplicate-target-a.mdoc', 'duplicate-target', 'Duplicate A', 'duplicate a'),
                 ('duplicate-target-b.mdoc', 'duplicate-target', 'Duplicate B', 'duplicate b');
             INSERT INTO mdoc_issues (path, kind, ref_fnode, error) VALUES
                 ('invalid-target.mdoc', 'invalid', 'invalid-target', 'invalid target'),
                 ('duplicate-target-a.mdoc', 'duplicate', 'duplicate-target', 'duplicate target'),
                 ('duplicate-target-b.mdoc', 'duplicate', 'duplicate-target', 'duplicate target'),
                 ('partial-target.mdoc', 'invalid', 'partial-target', 'partial target');",
        )
        .unwrap();

        let rows: Vec<(String, String)> = conn
            .prepare("SELECT path, ref_fnode FROM mdoc_missing_issues ORDER BY ref_fnode")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(
            rows,
            vec![
                ("<unknown>".into(), "absent-target".into()),
                ("<unknown>".into(), "partial-target".into()),
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

    #[test]
    fn schema_sixteen_is_rebuilt_with_formal_status_columns() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_mdoc_files_lean_verified;
             DROP INDEX idx_mdoc_files_rocq_verified;
             ALTER TABLE mdoc_files DROP COLUMN rocq_status;
             ALTER TABLE mdoc_files DROP COLUMN lean_status;
             PRAGMA user_version = 16;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();
        let status_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdoc_files')
                 WHERE name IN ('lean_status', 'rocq_status')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status_columns, 2);
    }

    #[test]
    fn formal_verified_statuses_have_partial_indexes() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();

        for (index, column) in [
            ("idx_mdoc_files_lean_verified", "lean_status"),
            ("idx_mdoc_files_rocq_verified", "rocq_status"),
        ] {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(sql.contains(column));
            assert!(sql.contains("WHERE"));
        }
    }

    #[test]
    fn schema_seventeen_migrates_without_discarding_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_files (path, mtime_ns, size) VALUES ('source.mdoc', 1, 2);
             INSERT INTO mdocs (path, fnode, title, title_lc)
                 VALUES ('source.mdoc', 'source-node', 'Source', 'source');
             INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                  VALUES ('source.mdoc', 'source-node', 'target-node', 0);
             INSERT INTO mdoc_in_degree (fnode, in_degree) VALUES
                  ('target-node', 99),
                  ('stale-target', 1);
             INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
                  VALUES ('source.mdoc', 'missing', 'target-node', 'legacy missing target');
             DROP VIEW mdoc_missing_issues;
             DROP INDEX idx_mdoc_edges_src_dst;
             DROP INDEX idx_mdoc_edges_dst_src;
             PRAGMA user_version = 17;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        let edge: (String, String) = conn
            .query_row(
                "SELECT src_fnode, dst_fnode FROM mdoc_edges WHERE src_path = 'source.mdoc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(edge, ("source-node".into(), "target-node".into()));
        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let stored_missing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mdoc_issues WHERE kind = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_missing, 0);
        let derived_missing: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_missing_issues", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(derived_missing, 1);
        for index in ["idx_mdoc_edges_src_dst", "idx_mdoc_edges_dst_src"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = ?)",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists);
        }
    }

    #[test]
    fn schema_eighteen_migrates_missing_issues_without_discarding_base_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_files (path, mtime_ns, size) VALUES
                 ('source.mdoc', 1, 2),
                 ('bad.mdoc', 3, 4);
             INSERT INTO mdocs (path, fnode, title, title_lc)
                  VALUES ('source.mdoc', 'source-node', 'Source', 'source');
             INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                  VALUES ('source.mdoc', 'source-node', 'target-node', 0);
             INSERT INTO mdoc_in_degree (fnode, in_degree) VALUES
                  ('target-node', 99),
                  ('stale-target', 1);
             INSERT INTO mdoc_issues (path, kind, ref_fnode, error) VALUES
                  ('source.mdoc', 'missing', 'target-node', 'legacy missing target'),
                  ('bad.mdoc', 'invalid', 'bad-node', 'invalid node');
             DROP VIEW mdoc_missing_issues;
             PRAGMA user_version = 18;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT kind, ref_fnode FROM mdoc_issues ORDER BY kind, ref_fnode")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, vec![("invalid".into(), "bad-node".into())]);
        let derived: (String, String) = conn
            .query_row(
                "SELECT path, ref_fnode FROM mdoc_missing_issues",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(derived, ("<unknown>".into(), "target-node".into()));
        let degrees: Vec<(String, i64)> = conn
            .prepare("SELECT fnode, in_degree FROM mdoc_in_degree ORDER BY fnode")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(degrees, vec![("target-node".into(), 1)]);
        let base_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(base_rows, 2);
    }

    #[test]
    fn failed_missing_issue_migration_rolls_back_schema_and_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
                  VALUES ('source.mdoc', 'missing', 'target-node', 'legacy missing target');
             DROP VIEW mdoc_missing_issues;
             CREATE TRIGGER reject_missing_issue_delete
             BEFORE DELETE ON mdoc_issues
             WHEN OLD.kind = 'missing'
             BEGIN
                 SELECT RAISE(ABORT, 'preserve missing row');
             END;
             PRAGMA user_version = 18;",
        )
        .unwrap();
        drop(conn);

        assert!(open_db(&path).is_err());

        let conn = Connection::open(&path).unwrap();
        assert_eq!(checked_user_version(&conn).unwrap(), 18);
        let missing_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mdoc_issues WHERE kind = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing_rows, 1);
        let view_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_schema
                     WHERE type = 'view' AND name = 'mdoc_missing_issues'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!view_exists);
    }

    #[test]
    fn failed_incremental_migration_rolls_back_schema_and_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                 VALUES ('source.mdoc', 'source-node', 'target-node', 0);
             DROP INDEX idx_mdoc_edges_src_dst;
             DROP INDEX idx_mdoc_edges_dst_src;
             CREATE TABLE idx_mdoc_edges_dst_src (value TEXT);
             PRAGMA user_version = 17;",
        )
        .unwrap();
        drop(conn);

        assert!(open_db(&path).is_err());

        let conn = Connection::open(&path).unwrap();
        assert_eq!(checked_user_version(&conn).unwrap(), 17);
        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(edges, 1);
        let first_index_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_schema
                     WHERE type = 'index' AND name = 'idx_mdoc_edges_src_dst'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!first_index_exists);
    }

    #[test]
    fn edge_pair_queries_use_covering_indexes() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();

        for (sql, index) in [
            (
                "SELECT dst_fnode, src_path FROM mdoc_edges WHERE src_fnode = 'source'",
                "idx_mdoc_edges_src_dst",
            ),
            (
                "SELECT src_fnode, src_path, ord FROM mdoc_edges WHERE dst_fnode = 'target'",
                "idx_mdoc_edges_dst_src",
            ),
        ] {
            let plan: String = conn
                .query_row(&format!("EXPLAIN QUERY PLAN {sql}"), [], |row| row.get(3))
                .unwrap();
            assert!(plan.contains(index), "unexpected query plan: {plan}");
        }

        let plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT path, ref_fnode FROM mdoc_missing_issues
                 WHERE ref_fnode = 'target'",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("sqlite_autoindex_mdoc_in_degree_1"),
            "missing-target lookup did not use the in-degree key:\n{plan}"
        );
        assert!(
            plan.contains("idx_mdocs_fnode"),
            "missing-target lookup did not use the claimant index:\n{plan}"
        );
    }
}
