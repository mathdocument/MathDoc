use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

const SCHEMA_VERSION: i32 = 5;

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS mdocs (
    path       TEXT PRIMARY KEY,
    fnode      TEXT NOT NULL,
    title      TEXT NOT NULL,
    title_lc   TEXT NOT NULL,
    mtime_sec  INTEGER NOT NULL,
    mtime_ns   INTEGER NOT NULL,
    size       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc);
CREATE INDEX IF NOT EXISTS idx_mdocs_fnode    ON mdocs(fnode);

CREATE TABLE IF NOT EXISTS mdoc_files (
    path      TEXT PRIMARY KEY,
    mtime_sec INTEGER NOT NULL,
    mtime_ns  INTEGER NOT NULL,
    size      INTEGER NOT NULL
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
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    bootstrapped          INTEGER NOT NULL DEFAULT 0,
    graph_epoch           INTEGER NOT NULL DEFAULT 0,
    weak_component_dirty  INTEGER NOT NULL DEFAULT 1
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

/// Open the database at `path` with WAL mode and apply the v5 schema.
pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    apply_schema(&conn)?;
    Ok(conn)
}

fn apply_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_SQL)?;

    let user_version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if user_version < SCHEMA_VERSION {
        // Add mtime_ns to mdocs if missing (legacy schema migration).
        let has_mtime_ns: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mdocs') WHERE name = 'mtime_ns'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_mtime_ns {
            conn.execute_batch(
                "ALTER TABLE mdocs ADD COLUMN mtime_ns INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        conn.execute_batch(BACKFILL_IN_DEGREE_SQL)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }

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
    fn open_twice_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        open_db(&path).unwrap();
        open_db(&path).unwrap(); // second open should not fail
    }

    #[test]
    fn backfill_migration_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        // Simulate an old database by resetting user_version, then re-apply
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        conn.execute_batch("INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord) VALUES ('a.mdoc', 'fa', 'fb', 0)").unwrap();
        apply_schema(&conn).unwrap();
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
        apply_schema(&conn).unwrap();
    }
}
