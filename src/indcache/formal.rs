//! SQLite adapter for formal evidence evaluation and status materialization.

use crate::core::FormalCodeStatus;
use crate::formal::{
    attestation::{self as formal_attestation, FormalAttestation, FormalAttestationManifest},
    status::{self, FormalStatusValidation, WorkspaceEvaluation},
    FormalCompilationReceipt,
};
use anyhow::Result;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::path::Path;

pub(super) fn refresh_index_statuses(
    conn: &Connection,
    root: &Path,
) -> Result<FormalStatusValidation> {
    let _profile = crate::profile::scope("formal_status::refresh_index_statuses");
    let loaded = match formal_attestation::load_for_status(root) {
        Ok(loaded) => loaded,
        Err(_) => {
            downgrade_verified_statuses(conn)?;
            return Ok(FormalStatusValidation::default());
        }
    };
    if !loaded.manifest.has_attestations() {
        downgrade_verified_statuses(conn)?;
        return Ok(FormalStatusValidation::default());
    }
    let evaluation =
        match evaluate_workspace(conn, root, &loaded.manifest, None).and_then(|evaluation| {
            evaluation.ensure_current()?;
            formal_attestation::require_snapshot_current(root, &loaded.snapshot)?;
            Ok(evaluation)
        }) {
            Ok(evaluation) => evaluation,
            Err(error) if error.chain().any(|cause| cause.is::<rusqlite::Error>()) => {
                return Err(error)
            }
            Err(_) => {
                downgrade_verified_statuses(conn)?;
                return Ok(FormalStatusValidation::default());
            }
        };

    downgrade_verified_statuses(conn)?;
    let mut update = conn.prepare(
        "UPDATE mdoc_files SET lean_status = ?, rocq_status = ?
         WHERE path = ? AND (lean_status <> ? OR rocq_status <> ?)",
    )?;
    for (rel_path, lean, rocq) in evaluation.statuses() {
        let lean = status_value(lean);
        let rocq = status_value(rocq);
        update.execute(rusqlite::params![lean, rocq, rel_path, lean, rocq])?;
    }
    Ok(evaluation.finish_validation(loaded))
}

pub(super) fn downgrade_verified_statuses(conn: &Connection) -> Result<()> {
    // Keep the index usable without retaining any previously verified state.
    conn.execute(
        "UPDATE mdoc_files SET lean_status = 1 WHERE lean_status = 2",
        [],
    )?;
    conn.execute(
        "UPDATE mdoc_files SET rocq_status = 1 WHERE rocq_status = 2",
        [],
    )?;
    Ok(())
}

fn evaluate_workspace(
    conn: &Connection,
    root: &Path,
    manifest: &FormalAttestationManifest,
    required_fnode: Option<&str>,
) -> Result<WorkspaceEvaluation> {
    let mut required = manifest.nodes.keys().cloned().collect::<BTreeSet<_>>();
    required.extend(required_fnode.map(str::to_string));
    let locations = valid_locations(conn, &required.into_iter().collect::<Vec<_>>())?;
    let collected = status::collect_workspace(root, locations)?;
    let dependencies = valid_locations(conn, &collected.dependency_refs())?;
    status::evaluate_workspace(collected, manifest, dependencies)
}

pub(super) fn prepare_attestation(
    conn: &Connection,
    root: &Path,
    manifest: &FormalAttestationManifest,
    fnode: &str,
    language: &str,
    receipt: &FormalCompilationReceipt,
) -> Result<FormalAttestation> {
    let evaluation = evaluate_workspace(conn, root, manifest, Some(fnode))?;
    status::prepare_attestation(evaluation, root, manifest, fnode, language, receipt)
}

fn valid_locations(conn: &Connection, fnodes: &[String]) -> Result<Vec<(String, String)>> {
    let mut rows = Vec::with_capacity(fnodes.len());
    for chunk in fnodes.chunks(super::queries::CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT m.path, m.fnode
             FROM mdocs m
             WHERE m.fnode IN ({placeholders})
               AND NOT EXISTS (
                   SELECT 1 FROM mdoc_issues i
                   WHERE i.path = m.path AND i.kind IN ('invalid', 'duplicate')
               )"
        );
        let mut stmt = conn.prepare(&sql)?;
        rows.extend(
            stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(rows)
}

fn status_value(status: FormalCodeStatus) -> i64 {
    match status {
        FormalCodeStatus::NoCode => 0,
        FormalCodeStatus::Unverified => 1,
        FormalCodeStatus::Verified => 2,
    }
}
