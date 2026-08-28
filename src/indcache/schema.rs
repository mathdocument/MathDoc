use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

const SCHEMA_VERSION: i32 = 23;
const FIRST_INCREMENTAL_SCHEMA_VERSION: i32 = 17;
const MIN_SQLITE_VERSION_NUMBER: i32 = 3_051_003;

const CREATE_SQL: &str = "
CREATE TABLE IF NOT EXISTS mdocs (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    fnode       TEXT    NOT NULL,
    fnode_lc    TEXT    GENERATED ALWAYS AS (lower(fnode)) VIRTUAL,
    title       TEXT    NOT NULL,
    title_lc    TEXT    NOT NULL,
    topo_depth  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc);
CREATE INDEX IF NOT EXISTS idx_mdocs_fnode    ON mdocs(fnode);
CREATE INDEX IF NOT EXISTS idx_mdocs_fnode_lc ON mdocs(fnode_lc);

CREATE VIRTUAL TABLE IF NOT EXISTS mdoc_search USING fts5(
    fnode_lc,
    title_lc,
    content = 'mdocs',
    content_rowid = 'id',
    tokenize = 'trigram case_sensitive 1',
    detail = none,
    columnsize = 0
);
CREATE VIRTUAL TABLE IF NOT EXISTS mdoc_search_vocab USING fts5vocab(mdoc_search, 'row');
CREATE TRIGGER IF NOT EXISTS mdocs_search_insert AFTER INSERT ON mdocs BEGIN
    INSERT INTO mdoc_search(rowid, fnode_lc, title_lc)
    VALUES (new.id, new.fnode_lc, new.title_lc);
END;
CREATE TRIGGER IF NOT EXISTS mdocs_search_delete AFTER DELETE ON mdocs BEGIN
    INSERT INTO mdoc_search(mdoc_search, rowid, fnode_lc, title_lc)
    VALUES ('delete', old.id, old.fnode_lc, old.title_lc);
END;
CREATE TRIGGER IF NOT EXISTS mdocs_search_update
AFTER UPDATE OF fnode, title_lc ON mdocs BEGIN
    INSERT INTO mdoc_search(mdoc_search, rowid, fnode_lc, title_lc)
    VALUES ('delete', old.id, old.fnode_lc, old.title_lc);
    INSERT INTO mdoc_search(rowid, fnode_lc, title_lc)
    VALUES (new.id, new.fnode_lc, new.title_lc);
END;

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

CREATE TABLE IF NOT EXISTS mdoc_symbols (
    id    INTEGER PRIMARY KEY,
    fnode TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS mdoc_workdraft_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    manifest_digest BLOB    NOT NULL DEFAULT X'',
    valid_mdocs     INTEGER NOT NULL DEFAULT 0 CHECK (valid_mdocs >= 0),
    source_files    INTEGER NOT NULL DEFAULT 0 CHECK (source_files >= 0)
);
INSERT OR IGNORE INTO mdoc_workdraft_state (id) VALUES (1);

CREATE TABLE IF NOT EXISTS mdoc_workdraft_observations (
    source_id  TEXT    NOT NULL,
    srctype    TEXT    NOT NULL,
    present    INTEGER NOT NULL CHECK (present IN (0, 1)),
    device     INTEGER NOT NULL,
    inode      INTEGER NOT NULL,
    size       INTEGER NOT NULL,
    mtime      INTEGER NOT NULL,
    mtime_nsec INTEGER NOT NULL,
    ctime      INTEGER NOT NULL,
    ctime_nsec INTEGER NOT NULL,
    mode       INTEGER NOT NULL,
    uid        INTEGER NOT NULL,
    gid        INTEGER NOT NULL,
    PRIMARY KEY (source_id, srctype)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS mdoc_edges (
    src_path      TEXT    NOT NULL,
    src_symbol_id INTEGER NOT NULL REFERENCES mdoc_symbols(id),
    dst_symbol_id INTEGER NOT NULL REFERENCES mdoc_symbols(id),
    ord           INTEGER NOT NULL,
    PRIMARY KEY (src_path, ord)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_dst
    ON mdoc_edges(src_symbol_id, dst_symbol_id, src_path);
CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_src
    ON mdoc_edges(dst_symbol_id, src_symbol_id, src_path, ord);

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
SELECT e.src_path, src.fnode AS src_fnode, dst.fnode AS dst_fnode, e.ord
FROM mdoc_edges e
JOIN mdoc_symbols src ON src.id = e.src_symbol_id
JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
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
    index_digest         TEXT    NOT NULL DEFAULT '',
    document_count       INTEGER NOT NULL DEFAULT 0 CHECK (document_count >= 0),
    index_dirty          INTEGER NOT NULL DEFAULT 0 CHECK (index_dirty IN (0, 1))
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
DROP TRIGGER IF EXISTS mdocs_search_update;
DROP TRIGGER IF EXISTS mdocs_search_delete;
DROP TRIGGER IF EXISTS mdocs_search_insert;
DROP TABLE IF EXISTS mdoc_search_vocab;
DROP TABLE IF EXISTS mdoc_search;
DROP VIEW IF EXISTS mdoc_missing_issues;
DROP VIEW IF EXISTS mdoc_valid_edges;
DROP TABLE IF EXISTS mdoc_weak_component;
DROP TABLE IF EXISTS mdoc_scc_result;
DROP TABLE IF EXISTS mdoc_in_degree;
DROP TABLE IF EXISTS mdoc_index_state;
DROP TABLE IF EXISTS mdoc_issues;
DROP TABLE IF EXISTS mdoc_edges;
DROP TABLE IF EXISTS mdoc_symbols;
DROP TABLE IF EXISTS mdoc_workdraft_observations;
DROP TABLE IF EXISTS mdoc_workdraft_state;
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

const MIGRATE_19_TO_20_SQL: &str = "
DROP VIEW IF EXISTS mdoc_missing_issues;
ALTER TABLE mdocs RENAME TO mdocs_schema_19;
CREATE TABLE mdocs (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    fnode       TEXT    NOT NULL,
    fnode_lc    TEXT    GENERATED ALWAYS AS (lower(fnode)) VIRTUAL,
    title       TEXT    NOT NULL,
    title_lc    TEXT    NOT NULL,
    topo_depth  INTEGER NOT NULL DEFAULT 0
);
INSERT INTO mdocs (path, fnode, title, title_lc, topo_depth)
SELECT path, fnode, title, title_lc, topo_depth
FROM mdocs_schema_19
ORDER BY path;
DROP TABLE mdocs_schema_19;
ALTER TABLE mdoc_index_state
    ADD COLUMN document_count INTEGER NOT NULL DEFAULT 0 CHECK (document_count >= 0);
UPDATE mdoc_index_state
SET document_count = (SELECT COUNT(*) FROM mdocs)
WHERE id = 1;
";

const MIGRATE_20_TO_21_SQL: &str = "
DROP VIEW IF EXISTS mdoc_valid_edges;
ALTER TABLE mdoc_edges RENAME TO mdoc_edges_schema_20;
CREATE TABLE mdoc_symbols (
    id    INTEGER PRIMARY KEY,
    fnode TEXT NOT NULL UNIQUE
);
INSERT INTO mdoc_symbols (fnode)
SELECT fnode FROM (
    SELECT src_fnode AS fnode FROM mdoc_edges_schema_20
    UNION
    SELECT dst_fnode AS fnode FROM mdoc_edges_schema_20
)
ORDER BY fnode;
CREATE TABLE mdoc_edges (
    src_path      TEXT    NOT NULL,
    src_symbol_id INTEGER NOT NULL REFERENCES mdoc_symbols(id),
    dst_symbol_id INTEGER NOT NULL REFERENCES mdoc_symbols(id),
    ord           INTEGER NOT NULL,
    PRIMARY KEY (src_path, ord)
) WITHOUT ROWID;
INSERT INTO mdoc_edges (src_path, src_symbol_id, dst_symbol_id, ord)
SELECT e.src_path, src.id, dst.id, e.ord
FROM mdoc_edges_schema_20 e
JOIN mdoc_symbols src ON src.fnode = e.src_fnode
JOIN mdoc_symbols dst ON dst.fnode = e.dst_fnode
ORDER BY e.src_path, e.ord;
DROP TABLE mdoc_edges_schema_20;
";

/// Open the database at `path` with WAL mode and apply the schema.
pub(super) fn open_db(path: &Path) -> Result<Connection> {
    ensure_supported_sqlite()?;
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

fn ensure_supported_sqlite() -> Result<()> {
    let version = rusqlite::version_number();
    if version < MIN_SQLITE_VERSION_NUMBER {
        bail!(
            "SQLite {} is unsupported; MathDoc requires 3.51.3 or newer",
            rusqlite::version()
        );
    }
    Ok(())
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
            user_version = 19;
        }
        let rebuild_search = user_version == 19;
        if rebuild_search {
            tx.execute_batch(MIGRATE_19_TO_20_SQL)?;
            user_version = 20;
        }
        if user_version == 20 {
            tx.execute_batch(MIGRATE_20_TO_21_SQL)?;
        }
        tx.execute_batch(CREATE_SQL)?;
        if user_version < 23 {
            migrate_index_dirty(&tx)?;
        }
        if rebuild_search {
            tx.execute(
                "INSERT INTO mdoc_search(mdoc_search) VALUES ('rebuild')",
                [],
            )?;
        }
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }

    tx.commit()?;
    Ok(())
}

fn migrate_index_dirty(conn: &Connection) -> Result<()> {
    let index_has_marker: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('mdoc_index_state') WHERE name = 'index_dirty'
         )",
        [],
        |row| row.get(0),
    )?;
    if !index_has_marker {
        conn.execute_batch(
            "ALTER TABLE mdoc_index_state
                 ADD COLUMN index_dirty INTEGER NOT NULL DEFAULT 0
                 CHECK (index_dirty IN (0, 1));",
        )?;
    }
    let workdraft_has_marker: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('mdoc_workdraft_state') WHERE name = 'index_dirty'
         )",
        [],
        |row| row.get(0),
    )?;
    if workdraft_has_marker {
        conn.execute_batch(
            "UPDATE mdoc_index_state
             SET index_dirty = (SELECT index_dirty FROM mdoc_workdraft_state WHERE id = 1)
             WHERE id = 1;
             ALTER TABLE mdoc_workdraft_state DROP COLUMN index_dirty;",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn insert_edge(conn: &Connection, path: &str, source: &str, target: &str, order: i64) {
        conn.execute(
            "INSERT INTO mdoc_symbols (fnode) VALUES (?), (?)
             ON CONFLICT(fnode) DO NOTHING",
            [source, target],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mdoc_edges (src_path, src_symbol_id, dst_symbol_id, ord)
             SELECT ?, src.id, dst.id, ?
             FROM mdoc_symbols src, mdoc_symbols dst
             WHERE src.fnode = ? AND dst.fnode = ?",
            rusqlite::params![path, order, source, target],
        )
        .unwrap();
    }

    fn downgrade_edges_to_schema_twenty(conn: &Connection) {
        conn.execute_batch(
            "DROP VIEW mdoc_valid_edges;
             ALTER TABLE mdoc_edges RENAME TO mdoc_edges_schema_21;
             CREATE TABLE mdoc_edges (
                 src_path  TEXT    NOT NULL,
                 src_fnode TEXT    NOT NULL,
                 dst_fnode TEXT    NOT NULL,
                 ord       INTEGER NOT NULL,
                 PRIMARY KEY (src_path, ord)
             );
             INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
             SELECT e.src_path, src.fnode, dst.fnode, e.ord
             FROM mdoc_edges_schema_21 e
             JOIN mdoc_symbols src ON src.id = e.src_symbol_id
             JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id;
             DROP TABLE mdoc_edges_schema_21;
             DROP TABLE mdoc_symbols;
             CREATE INDEX idx_mdoc_edges_src_fnode ON mdoc_edges(src_fnode);
             CREATE INDEX idx_mdoc_edges_dst_fnode ON mdoc_edges(dst_fnode);
             CREATE INDEX idx_mdoc_edges_src_dst
                 ON mdoc_edges(src_fnode, dst_fnode, src_path);
             CREATE INDEX idx_mdoc_edges_dst_src
                 ON mdoc_edges(dst_fnode, src_fnode, src_path, ord);
             CREATE VIEW mdoc_valid_edges AS
             SELECT e.src_path, e.src_fnode, e.dst_fnode, e.ord
             FROM mdoc_edges e
             WHERE NOT EXISTS (
                 SELECT 1 FROM mdoc_issues i
                 WHERE i.path = e.src_path AND i.kind IN ('invalid', 'duplicate')
             );",
        )
        .unwrap();
    }

    fn downgrade_mdocs_to_schema_nineteen(conn: &Connection) {
        conn.execute_batch(
            "DROP TRIGGER mdocs_search_update;
             DROP TRIGGER mdocs_search_delete;
             DROP TRIGGER mdocs_search_insert;
             DROP TABLE mdoc_search_vocab;
             DROP TABLE mdoc_search;
             DROP VIEW mdoc_missing_issues;
             ALTER TABLE mdocs RENAME TO mdocs_schema_20;
             CREATE TABLE mdocs (
                 path       TEXT PRIMARY KEY,
                 fnode      TEXT NOT NULL,
                 title      TEXT NOT NULL,
                 title_lc   TEXT NOT NULL,
                 topo_depth INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO mdocs (path, fnode, title, title_lc, topo_depth)
             SELECT path, fnode, title, title_lc, topo_depth FROM mdocs_schema_20;
             DROP TABLE mdocs_schema_20;
             ALTER TABLE mdoc_index_state DROP COLUMN document_count;",
        )
        .unwrap();
    }

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
    fn bundled_sqlite_has_required_engine_capabilities() {
        ensure_supported_sqlite().unwrap();
        assert!(rusqlite::version_number() >= MIN_SQLITE_VERSION_NUMBER);

        let conn = Connection::open_in_memory().unwrap();
        let fts5_enabled: bool = conn
            .query_row(
                "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fts5_enabled, "bundled SQLite must include FTS5");
    }

    #[test]
    fn valid_edge_view_filters_only_blocking_source_issues() {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("index.db")).unwrap();
        for (path, source) in [
            ("valid.mdoc", "valid"),
            ("missing.mdoc", "missing"),
            ("invalid.mdoc", "invalid"),
            ("duplicate.mdoc", "duplicate"),
        ] {
            insert_edge(&conn, path, source, "target", 0);
        }
        conn.execute_batch(
            "INSERT INTO mdoc_issues (path, kind, ref_fnode, error) VALUES
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
        insert_edge(&conn, "a.mdoc", "fa", "fb", 0);
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
        downgrade_edges_to_schema_twenty(&conn);
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
             ALTER TABLE mdoc_index_state DROP COLUMN document_count;
             PRAGMA user_version = 17;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        let edge: (String, String) = conn
            .query_row(
                "SELECT src_fnode, dst_fnode
                 FROM mdoc_valid_edges WHERE src_path = 'source.mdoc'",
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
        downgrade_edges_to_schema_twenty(&conn);
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
             ALTER TABLE mdoc_index_state DROP COLUMN document_count;
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
    fn schema_nineteen_builds_the_search_index_without_discarding_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO mdocs (path, fnode, title, title_lc)
                   VALUES ('source.mdoc', 'SOURCE-NODE', 'Search Needle', 'search needle');",
        )
        .unwrap();
        downgrade_mdocs_to_schema_nineteen(&conn);
        downgrade_edges_to_schema_twenty(&conn);
        conn.execute_batch("PRAGMA user_version = 19;").unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let matched: String = conn
            .query_row(
                "SELECT m.path
                 FROM mdoc_search s JOIN mdocs m ON m.id = s.rowid
                 WHERE mdoc_search MATCH '\"nee\" AND \"eed\" AND \"edl\" AND \"dle\"'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(matched, "source.mdoc");
        let identity: (i64, String) = conn
            .query_row(
                "SELECT id, fnode_lc FROM mdocs WHERE path = 'source.mdoc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(identity.1, "source-node");

        conn.execute_batch("VACUUM;").unwrap();
        let id_after_vacuum: i64 = conn
            .query_row(
                "SELECT id FROM mdocs WHERE path = 'source.mdoc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(id_after_vacuum, identity.0);
        let still_indexed: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM mdoc_search
                     WHERE mdoc_search MATCH '\"sou\" AND \"our\" AND \"urc\" AND \"rce\"'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(still_indexed);
    }

    #[test]
    fn schema_twenty_normalizes_edge_endpoints_without_discarding_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        downgrade_edges_to_schema_twenty(&conn);
        conn.execute_batch(
            "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord) VALUES
                 ('a.mdoc', 'shared-source', 'first-target', 0),
                 ('b.mdoc', 'shared-source', 'second-target', 0);
             INSERT INTO mdoc_issues (path, kind, ref_fnode, error)
                 VALUES ('a.mdoc', 'invalid', 'shared-source', 'blocked source');
             INSERT INTO mdoc_in_degree (fnode, in_degree)
                 VALUES ('derived-sentinel', 7);
             PRAGMA user_version = 20;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let edges: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT e.src_path, src.fnode, dst.fnode
                 FROM mdoc_edges e
                 JOIN mdoc_symbols src ON src.id = e.src_symbol_id
                 JOIN mdoc_symbols dst ON dst.id = e.dst_symbol_id
                 ORDER BY e.src_path, e.ord",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            edges,
            [
                (
                    "a.mdoc".into(),
                    "shared-source".into(),
                    "first-target".into()
                ),
                (
                    "b.mdoc".into(),
                    "shared-source".into(),
                    "second-target".into()
                ),
            ]
        );
        let symbol_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_symbols", [], |row| row.get(0))
            .unwrap();
        assert_eq!(symbol_count, 3);
        let valid_edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_valid_edges", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(valid_edge_count, 1);
        let derived_degree: i64 = conn
            .query_row(
                "SELECT in_degree FROM mdoc_in_degree WHERE fnode = 'derived-sentinel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_degree, 7);
        let edge_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'mdoc_edges'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(edge_sql.contains("WITHOUT ROWID"));
    }

    #[test]
    fn schema_twenty_one_adds_workdraft_observations_without_discarding_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        insert_edge(&conn, "source.mdoc", "source-node", "target-node", 0);
        conn.execute_batch(
            "DROP TABLE mdoc_workdraft_observations;
             DROP TABLE mdoc_workdraft_state;
             PRAGMA user_version = 21;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(edges, 1);
        let state_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdoc_workdraft_state", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state_rows, 1);
    }

    #[test]
    fn schema_twenty_two_moves_the_dirty_marker_without_discarding_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        conn.execute(
            "INSERT INTO mdocs (path, fnode, title, title_lc) VALUES (?, ?, ?, ?)",
            ["node.mdoc", "node", "Node", "node"],
        )
        .unwrap();
        conn.execute_batch(
            "ALTER TABLE mdoc_workdraft_state
                 ADD COLUMN index_dirty INTEGER NOT NULL DEFAULT 0
                 CHECK (index_dirty IN (0, 1));
             UPDATE mdoc_workdraft_state SET index_dirty = 1 WHERE id = 1;
             ALTER TABLE mdoc_index_state DROP COLUMN index_dirty;
             PRAGMA user_version = 22;",
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();

        assert_eq!(checked_user_version(&conn).unwrap(), SCHEMA_VERSION);
        let dirty: bool = conn
            .query_row(
                "SELECT index_dirty FROM mdoc_index_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(dirty);
        let workdraft_has_marker: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('mdoc_workdraft_state')
                     WHERE name = 'index_dirty'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!workdraft_has_marker);
        let nodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM mdocs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(nodes, 1);
    }

    #[test]
    fn failed_edge_normalization_migration_rolls_back_schema_and_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        downgrade_edges_to_schema_twenty(&conn);
        conn.execute_batch(
            "INSERT INTO mdoc_edges (src_path, src_fnode, dst_fnode, ord)
                 VALUES ('source.mdoc', 'source-node', 'target-node', 0);
             DROP INDEX idx_mdoc_edges_dst_src;
             CREATE TABLE idx_mdoc_edges_dst_src (value TEXT);
             INSERT INTO idx_mdoc_edges_dst_src VALUES ('preserve me');
             PRAGMA user_version = 20;",
        )
        .unwrap();
        drop(conn);

        assert!(open_db(&path).is_err());

        let conn = Connection::open(&path).unwrap();
        assert_eq!(checked_user_version(&conn).unwrap(), 20);
        let edge: (String, String) = conn
            .query_row("SELECT src_fnode, dst_fnode FROM mdoc_edges", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(edge, ("source-node".into(), "target-node".into()));
        let symbols_exist: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_schema
                     WHERE type = 'table' AND name = 'mdoc_symbols'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!symbols_exist);
        let value: String = conn
            .query_row("SELECT value FROM idx_mdoc_edges_dst_src", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "preserve me");
    }

    #[test]
    fn failed_search_index_migration_rolls_back_schema_creation() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        downgrade_mdocs_to_schema_nineteen(&conn);
        conn.execute_batch(
            "CREATE TABLE mdoc_search (value TEXT);
             INSERT INTO mdoc_search VALUES ('preserve me');
             PRAGMA user_version = 19;",
        )
        .unwrap();
        drop(conn);

        assert!(open_db(&path).is_err());

        let conn = Connection::open(&path).unwrap();
        assert_eq!(checked_user_version(&conn).unwrap(), 19);
        let value: String = conn
            .query_row("SELECT value FROM mdoc_search", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "preserve me");
        let primary_key: String = conn
            .query_row(
                "SELECT name FROM pragma_table_info('mdocs') WHERE pk = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(primary_key, "path");
        let trigger_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'trigger' AND name LIKE 'mdocs_search_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 0);
    }

    #[test]
    fn failed_missing_issue_migration_rolls_back_schema_and_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.db");
        let conn = open_db(&path).unwrap();
        downgrade_edges_to_schema_twenty(&conn);
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
        downgrade_edges_to_schema_twenty(&conn);
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
                "SELECT dst_fnode, src_path
                 FROM mdoc_valid_edges WHERE src_fnode = 'source'",
                "idx_mdoc_edges_src_dst",
            ),
            (
                "SELECT src_fnode, src_path, ord
                 FROM mdoc_valid_edges WHERE dst_fnode = 'target'",
                "idx_mdoc_edges_dst_src",
            ),
        ] {
            let mut plan = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let plan = plan
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join("\n");
            assert!(plan.contains(index), "unexpected query plan:\n{plan}");
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

        let search_plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT rowid FROM mdoc_search
                 WHERE mdoc_search MATCH '\"nee\" AND \"eed\" AND \"edl\" AND \"dle\"'",
                [],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            search_plan.contains("VIRTUAL TABLE INDEX"),
            "search lookup did not use the FTS5 index: {search_plan}"
        );
    }
}
