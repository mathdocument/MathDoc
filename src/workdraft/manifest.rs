use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::config::builtin_srctypes;
use crate::workspace::FileSnapshot;

use super::mirror::validate_source_relative;

const MANIFEST_VERSION: u32 = 4;
pub(super) const MANIFEST_NAME: &str = "source-blocks.json";

pub(super) struct BlockBaseline([u8; 32]);

impl BlockBaseline {
    fn new(content: &[u8]) -> Self {
        Self(Sha256::digest(content).into())
    }

    fn matches(&self, content: &[u8]) -> bool {
        self.0 == <[u8; 32]>::from(Sha256::digest(content))
    }

    fn update(&mut self, content: &[u8]) {
        self.0 = Sha256::digest(content).into();
    }
}

impl Serialize for BlockBaseline {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_digest(&self.0))
    }
}

impl<'de> Deserialize<'de> for BlockBaseline {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_digest(&value)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("invalid digest in source block manifest"))
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct SourceBaseline {
    pub(super) blocks: BTreeMap<String, BlockBaseline>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    unknown: BTreeSet<String>,
}

impl Default for SourceBaseline {
    fn default() -> Self {
        Self {
            blocks: BTreeMap::new(),
            unknown: builtin_srctypes().map(str::to_string).collect(),
        }
    }
}

impl SourceBaseline {
    pub(super) fn is_unknown(&self, srctype: &str) -> bool {
        self.unknown.contains(srctype)
    }

    pub(super) fn is_present(&self, srctype: &str) -> bool {
        self.blocks.contains_key(srctype)
    }

    pub(super) fn has_established_types(&self) -> bool {
        builtin_srctypes().any(|srctype| !self.is_unknown(srctype))
    }

    pub(super) fn matches_state(&self, srctype: &str, content: &[u8], present: bool) -> bool {
        debug_assert!(!self.is_unknown(srctype));
        match self.blocks.get(srctype) {
            Some(baseline) => present && baseline.matches(content),
            None => !present,
        }
    }

    pub(super) fn matches_raw(&self, srctype: &str, content: Option<&[u8]>) -> bool {
        debug_assert!(!self.is_unknown(srctype));
        match (self.blocks.get(srctype), content) {
            (Some(baseline), Some(content)) => baseline.matches(content),
            (None, None) => true,
            _ => false,
        }
    }

    pub(super) fn update(&mut self, srctype: &str, content: &[u8], present: bool) {
        self.unknown.remove(srctype);
        if present {
            self.blocks
                .entry(srctype.to_string())
                .and_modify(|baseline| baseline.update(content))
                .or_insert_with(|| BlockBaseline::new(content));
        } else {
            self.blocks.remove(srctype);
        }
    }

    pub(super) fn forget(&mut self, srctype: &str) {
        self.blocks.remove(srctype);
        self.unknown.insert(srctype.to_string());
    }
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

#[derive(Deserialize)]
struct LegacyDenseBlockBaseline {
    digest: String,
    present: bool,
}

#[derive(Deserialize)]
struct LegacyDenseSourceBaseline {
    blocks: BTreeMap<String, LegacyDenseBlockBaseline>,
}

#[derive(Deserialize)]
struct LegacyDenseManifest {
    version: u32,
    sources: BTreeMap<String, LegacyDenseSourceBaseline>,
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
    if let Ok(manifest) = serde_json::from_slice::<SourceBlockManifest>(content) {
        if manifest.version == MANIFEST_VERSION {
            validate_manifest(&manifest, path)?;
            return Ok(LoadedManifest {
                manifest,
                legacy_sources: BTreeSet::new(),
                needs_sparse_migration: false,
            });
        }
    }
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
            let legacy: LegacyDenseManifest = serde_json::from_value(value)?;
            debug_assert_eq!(legacy.version, 2);
            validate_legacy_dense_manifest(&legacy, path)?;
            Ok(LoadedManifest {
                manifest: migrate_dense_manifest(legacy),
                legacy_sources: BTreeSet::new(),
                needs_sparse_migration: true,
            })
        }
        Some(3) => {
            let legacy: LegacyDenseManifest = serde_json::from_value(value)?;
            debug_assert_eq!(legacy.version, 3);
            validate_legacy_dense_manifest(&legacy, path)?;
            Ok(LoadedManifest {
                manifest: migrate_dense_manifest(legacy),
                legacy_sources: BTreeSet::new(),
                needs_sparse_migration: false,
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
        for srctype in source.blocks.keys() {
            validate_srctype(srctype, path)?;
        }
        for srctype in &source.unknown {
            validate_srctype(srctype, path)?;
            if source.blocks.contains_key(srctype) {
                bail!(
                    "source type {srctype:?} is both present and unknown in source block manifest {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn validate_legacy_dense_manifest(manifest: &LegacyDenseManifest, path: &Path) -> Result<()> {
    validate_source_ids(manifest.sources.keys(), path)?;
    for source in manifest.sources.values() {
        for (srctype, baseline) in &source.blocks {
            validate_srctype(srctype, path)?;
            validate_digest(srctype, &baseline.digest, path)?;
        }
    }
    Ok(())
}

fn validate_srctype(srctype: &str, path: &Path) -> Result<()> {
    if !builtin_srctypes().any(|known| known == srctype) {
        bail!(
            "invalid source type {srctype:?} in source block manifest {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_digest(srctype: &str, digest: &str, path: &Path) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!(
            "invalid digest for source type {srctype:?} in source block manifest {}",
            path.display()
        );
    }
    Ok(())
}

fn migrate_dense_manifest(legacy: LegacyDenseManifest) -> SourceBlockManifest {
    let sources = legacy
        .sources
        .into_iter()
        .map(|(source_id, source)| {
            let unknown = builtin_srctypes()
                .filter(|srctype| !source.blocks.contains_key(*srctype))
                .map(str::to_string)
                .collect();
            let blocks = source
                .blocks
                .into_iter()
                .filter_map(|(srctype, baseline)| {
                    baseline.present.then(|| {
                        let digest = decode_digest(&baseline.digest)
                            .expect("legacy manifest digest was validated");
                        (srctype, BlockBaseline(digest))
                    })
                })
                .collect();
            (source_id, SourceBaseline { blocks, unknown })
        })
        .collect();
    SourceBlockManifest {
        version: MANIFEST_VERSION,
        sources,
    }
}

fn encode_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn decode_digest(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        digest[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

fn validate_source_ids<'a>(
    source_ids: impl Iterator<Item = &'a String>,
    path: &Path,
) -> Result<()> {
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
    }
    Ok(())
}
