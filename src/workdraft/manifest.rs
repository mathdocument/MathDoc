use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::config::builtin_srctypes;
use crate::workspace::FileSnapshot;

use super::mirror::validate_source_relative;

const MANIFEST_VERSION: u32 = 3;
pub(super) const MANIFEST_NAME: &str = "source-blocks.json";

#[derive(Deserialize, Serialize)]
pub(super) struct BlockBaseline {
    digest: String,
    pub(super) present: bool,
}

impl BlockBaseline {
    pub(super) fn new(content: &[u8], present: bool) -> Self {
        Self {
            digest: Self::digest(content),
            present,
        }
    }

    pub(super) fn matches_state(&self, content: &[u8], present: bool) -> bool {
        present == self.present && Self::digest(content) == self.digest
    }

    pub(super) fn matches_raw(&self, content: Option<&[u8]>) -> bool {
        match content {
            Some(content) => self.present && Self::digest(content) == self.digest,
            None => !self.present,
        }
    }

    pub(super) fn update(&mut self, content: &[u8], present: bool) {
        self.digest = Self::digest(content);
        self.present = present;
    }

    fn digest(content: &[u8]) -> String {
        format!("{:x}", Sha256::digest(content))
    }
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct SourceBaseline {
    pub(super) blocks: BTreeMap<String, BlockBaseline>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct SourceBlockManifest {
    version: u32,
    pub(super) sources: BTreeMap<String, SourceBaseline>,
}

#[derive(Deserialize)]
struct LegacyManifest {
    version: u32,
    sources: BTreeSet<String>,
}

pub(super) struct LoadedManifest {
    pub(super) manifest: SourceBlockManifest,
    pub(super) legacy_sources: BTreeSet<String>,
    pub(super) needs_sparse_migration: bool,
}

pub(super) fn parse_manifest(snapshot: &FileSnapshot, path: &Path) -> Result<LoadedManifest> {
    let Some(content) = snapshot.content() else {
        return Ok(LoadedManifest {
            manifest: empty_manifest(),
            legacy_sources: BTreeSet::new(),
            needs_sparse_migration: false,
        });
    };
    let value: serde_json::Value = serde_json::from_slice(content)
        .with_context(|| format!("reading source block manifest {}", path.display()))?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => {
            let legacy: LegacyManifest = serde_json::from_value(value)?;
            debug_assert_eq!(legacy.version, 1);
            validate_source_ids(legacy.sources.iter(), path)?;
            Ok(LoadedManifest {
                manifest: empty_manifest(),
                legacy_sources: legacy.sources,
                needs_sparse_migration: true,
            })
        }
        Some(2) => {
            let mut manifest: SourceBlockManifest = serde_json::from_value(value)?;
            validate_manifest(&manifest, path)?;
            manifest.version = MANIFEST_VERSION;
            Ok(LoadedManifest {
                manifest,
                legacy_sources: BTreeSet::new(),
                needs_sparse_migration: true,
            })
        }
        Some(version) if version == MANIFEST_VERSION as u64 => {
            let manifest: SourceBlockManifest = serde_json::from_value(value)?;
            validate_manifest(&manifest, path)?;
            Ok(LoadedManifest {
                manifest,
                legacy_sources: BTreeSet::new(),
                needs_sparse_migration: false,
            })
        }
        Some(version) => bail!(
            "unsupported source block manifest version {version} in {}",
            path.display()
        ),
        None => bail!("source block manifest has no version: {}", path.display()),
    }
}

pub(super) fn encode_source_path(path: &Path) -> String {
    let bytes = path.as_os_str().as_bytes();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub(super) fn decode_source_path(encoded: &str) -> Result<PathBuf> {
    if !encoded.len().is_multiple_of(2) || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid path encoding in source block manifest");
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("validated ASCII hex");
            u8::from_str_radix(pair, 16).context("decoding source block manifest path")
        })
        .collect::<Result<Vec<_>>>()?;
    let path = PathBuf::from(OsString::from_vec(bytes));
    validate_source_relative(&path)?;
    Ok(path)
}

fn empty_manifest() -> SourceBlockManifest {
    SourceBlockManifest {
        version: MANIFEST_VERSION,
        sources: BTreeMap::new(),
    }
}

fn validate_manifest(manifest: &SourceBlockManifest, path: &Path) -> Result<()> {
    validate_source_ids(manifest.sources.keys(), path)?;
    for source in manifest.sources.values() {
        for (srctype, baseline) in &source.blocks {
            if !builtin_srctypes().any(|known| known == srctype) {
                bail!(
                    "invalid source type {srctype:?} in source block manifest {}",
                    path.display()
                );
            }
            if baseline.digest.len() != 64
                || !baseline
                    .digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                bail!(
                    "invalid digest for source type {srctype:?} in source block manifest {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn validate_source_ids<'a>(
    source_ids: impl Iterator<Item = &'a String>,
    path: &Path,
) -> Result<()> {
    let mut decoded_paths = BTreeSet::new();
    for source_id in source_ids {
        let decoded = decode_source_path(source_id).with_context(|| {
            format!(
                "invalid source path in source block manifest {}",
                path.display()
            )
        })?;
        if encode_source_path(&decoded) != *source_id {
            bail!(
                "noncanonical source path encoding in source block manifest {}",
                path.display()
            );
        }
        if !decoded_paths.insert(decoded) {
            bail!(
                "duplicate source path in source block manifest {}",
                path.display()
            );
        }
    }
    Ok(())
}
