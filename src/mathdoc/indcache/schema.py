import sqlite3


def ensure_index_schema(
    conn: sqlite3.Connection,
    *,
    schema_version: int,
) -> None:
    user_version = int(conn.execute("PRAGMA user_version").fetchone()[0])
    column_rows = conn.execute("PRAGMA table_info(mdocs)").fetchall()
    if not column_rows:
        create_index_table(conn)
        column_rows = conn.execute("PRAGMA table_info(mdocs)").fetchall()

    columns = {str(row[1]) for row in column_rows}
    fnode_is_primary = any(
        str(row[1]) == "fnode" and int(row[5]) == 1 for row in column_rows
    )
    path_is_primary = any(
        str(row[1]) == "path" and int(row[5]) == 1 for row in column_rows
    )
    if fnode_is_primary or not path_is_primary:
        conn.execute("DROP TABLE mdocs")
        create_index_table(conn)
        columns = {
            str(row[1])
            for row in conn.execute("PRAGMA table_info(mdocs)").fetchall()
        }
    if "mtime_ns" not in columns:
        if "mtime_sec" not in columns:
            raise sqlite3.DatabaseError(
                "mdocs table is missing required mtime columns"
            )
        conn.execute(
            "ALTER TABLE mdocs ADD COLUMN mtime_ns INTEGER NOT NULL DEFAULT 0"
        )
        conn.execute(
            "UPDATE mdocs SET mtime_ns = mtime_sec * 1000000000 WHERE mtime_ns = 0"
        )
        columns.add("mtime_ns")
    if "mtime_sec" not in columns:
        conn.execute(
            "ALTER TABLE mdocs ADD COLUMN mtime_sec INTEGER NOT NULL DEFAULT 0"
        )
        conn.execute(
            "UPDATE mdocs SET mtime_sec = CAST(mtime_ns / 1000000000 AS INTEGER)"
        )

    create_file_state_table(conn)
    create_dir_state_table(conn)
    create_edge_table(conn)
    create_issue_table(conn)
    create_index_state_table(conn)
    conn.execute("CREATE INDEX IF NOT EXISTS idx_mdocs_title_lc ON mdocs(title_lc)")
    conn.execute("CREATE INDEX IF NOT EXISTS idx_mdocs_fnode ON mdocs(fnode)")

    state_column_rows = conn.execute("PRAGMA table_info(mdoc_index_state)").fetchall()
    state_columns = {str(row[1]) for row in state_column_rows}
    if "graph_epoch" not in state_columns:
        conn.execute(
            "ALTER TABLE mdoc_index_state ADD COLUMN graph_epoch INTEGER NOT NULL DEFAULT 0"
        )
    if "weak_component_dirty" not in state_columns:
        conn.execute(
            "ALTER TABLE mdoc_index_state ADD COLUMN weak_component_dirty INTEGER NOT NULL DEFAULT 1"
        )

    create_in_degree_table(conn)
    create_scc_result_table(conn)
    create_weak_component_table(conn)

    if user_version < schema_version:
        if user_version < 5:
            # Backfill mdoc_in_degree from existing edges on migration to v5.
            # DELETE first so this block is idempotent if migration was interrupted.
            conn.execute("DELETE FROM mdoc_in_degree")
            conn.execute(
                """
                INSERT INTO mdoc_in_degree (fnode, in_degree)
                SELECT dst_fnode, COUNT(*)
                FROM mdoc_edges
                WHERE NOT EXISTS (
                    SELECT 1 FROM mdoc_issues
                    WHERE mdoc_issues.path = mdoc_edges.src_path
                      AND mdoc_issues.kind IN ('invalid', 'duplicate')
                )
                GROUP BY dst_fnode
                """
            )
        conn.execute(f"PRAGMA user_version = {schema_version}")


def create_index_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdocs (
            path TEXT PRIMARY KEY,
            fnode TEXT NOT NULL,
            title TEXT NOT NULL,
            title_lc TEXT NOT NULL,
            mtime_sec INTEGER NOT NULL,
            mtime_ns INTEGER NOT NULL,
            size INTEGER NOT NULL
        )
        """
    )


def create_file_state_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_files (
            path TEXT PRIMARY KEY,
            mtime_sec INTEGER NOT NULL,
            mtime_ns INTEGER NOT NULL,
            size INTEGER NOT NULL
        )
        """
    )


def create_dir_state_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_dirs (
            path TEXT PRIMARY KEY,
            mtime_ns INTEGER NOT NULL
        )
        """
    )


def create_edge_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_edges (
            src_path TEXT NOT NULL,
            src_fnode TEXT NOT NULL,
            dst_fnode TEXT NOT NULL,
            ord INTEGER NOT NULL,
            PRIMARY KEY (src_path, ord)
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_mdoc_edges_src_fnode ON mdoc_edges(src_fnode)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_mdoc_edges_dst_fnode ON mdoc_edges(dst_fnode)"
    )


def create_issue_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_issues (
            path TEXT NOT NULL,
            kind TEXT NOT NULL,
            ref_fnode TEXT NOT NULL,
            error TEXT NOT NULL,
            PRIMARY KEY (path, kind, ref_fnode)
        )
        """
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_mdoc_issues_kind ON mdoc_issues(kind)"
    )
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_mdoc_issues_ref_fnode ON mdoc_issues(ref_fnode)"
    )


def create_index_state_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_index_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            bootstrapped INTEGER NOT NULL DEFAULT 0,
            graph_epoch INTEGER NOT NULL DEFAULT 0,
            weak_component_dirty INTEGER NOT NULL DEFAULT 1
        )
        """
    )
    conn.execute(
        """
        INSERT OR IGNORE INTO mdoc_index_state (id, bootstrapped)
        VALUES (1, 0)
        """
    )


def create_in_degree_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_in_degree (
            fnode TEXT PRIMARY KEY,
            in_degree INTEGER NOT NULL DEFAULT 0
        )
        """
    )


def create_scc_result_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_scc_result (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            graph_epoch INTEGER NOT NULL DEFAULT -1,
            cycles_json TEXT NOT NULL DEFAULT '[]'
        )
        """
    )


def create_weak_component_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS mdoc_weak_component (
            fnode TEXT PRIMARY KEY,
            component_size INTEGER NOT NULL DEFAULT 1
        )
        """
    )
