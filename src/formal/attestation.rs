use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::workspace::{AppliedWrite, FileSnapshot};

use super::EVIDENCE_SCHEME_VERSION;

const MANIFEST_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "formal-attestations.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormalAttestation {
    pub(crate) fnode: String,
    pub(crate) rel_path: String,
    pub(crate) target_module: String,
    pub(crate) source_sha256: String,
    pub(crate) artifact_sha256: String,
    pub(crate) environment_sha256: String,
    pub(crate) compiler_path: String,
    pub(crate) compiler_sha256: String,
    pub(crate) workspace_modules: BTreeSet<String>,
    pub(crate) dependencies: BTreeMap<String, String>,
    pub(crate) external_dependencies: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeAttestations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lean: Option<FormalAttestation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rocq: Option<FormalAttestation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormalAttestationManifest {
    version: u32,
    #[serde(default = "legacy_evidence_scheme_version")]
    evidence_scheme_version: u32,
    pub(crate) nodes: BTreeMap<String, NodeAttestations>,
}

pub(crate) struct LoadedManifest {
    pub(crate) path: PathBuf,
    pub(crate) snapshot: FileSnapshot,
    pub(crate) manifest: FormalAttestationManifest,
}

impl FormalAttestationManifest {
    pub(crate) fn has_attestations(&self) -> bool {
        self.nodes
            .values()
            .any(|node| node.lean.is_some() || node.rocq.is_some())
    }

    pub(crate) fn has_attestation_for(&self, fnode: &str) -> bool {
        self.nodes
            .get(fnode)
            .is_some_and(|node| node.lean.is_some() || node.rocq.is_some())
    }

    pub(crate) fn get(&self, fnode: &str, language: &str) -> Option<&FormalAttestation> {
        let node = self.nodes.get(fnode)?;
        match language {
            "lean" => node.lean.as_ref(),
            "rocq" => node.rocq.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn set(
        &mut self,
        fnode: &str,
        language: &str,
        attestation: FormalAttestation,
    ) -> Result<()> {
        let node = self.nodes.entry(fnode.to_string()).or_default();
        match language {
            "lean" => node.lean = Some(attestation),
            "rocq" => node.rocq = Some(attestation),
            _ => bail!("unsupported formal language: {language}"),
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, fnode: &str, language: &str) -> Result<bool> {
        let Some(node) = self.nodes.get_mut(fnode) else {
            return Ok(false);
        };
        let removed = match language {
            "lean" => node.lean.take().is_some(),
            "rocq" => node.rocq.take().is_some(),
            _ => bail!("unsupported formal language: {language}"),
        };
        if node.lean.is_none() && node.rocq.is_none() {
            self.nodes.remove(fnode);
        }
        Ok(removed)
    }
}

pub(crate) fn load(root: &Path) -> Result<LoadedManifest> {
    load_with_policy(root, true)
}

pub(crate) fn snapshot(root: &Path) -> Result<FileSnapshot> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    FileSnapshot::capture_beneath(&root, &root.join(".mdc").join(MANIFEST_NAME))
}

pub(crate) fn require_snapshot_current(root: &Path, snapshot: &FileSnapshot) -> Result<()> {
    let path = root.join(".mdc").join(MANIFEST_NAME);
    if snapshot.unchanged_beneath(root, &path)? {
        Ok(())
    } else {
        bail!("formal attestation manifest changed during compilation")
    }
}

pub(crate) fn load_for_status(root: &Path) -> Result<LoadedManifest> {
    load_with_policy(root, false)
}

fn load_with_policy(root: &Path, strict: bool) -> Result<LoadedManifest> {
    let path = root.join(".mdc").join(MANIFEST_NAME);
    let snapshot = FileSnapshot::capture_beneath(root, &path)?;
    let manifest = match snapshot.content() {
        None => empty_manifest(),
        Some(content) => {
            let parsed = serde_json::from_slice(content)
                .with_context(|| format!("reading formal attestation manifest {}", path.display()))
                .and_then(|manifest| {
                    validate_manifest(&manifest, &path)?;
                    Ok(manifest)
                });
            match parsed {
                Ok(manifest) => manifest,
                Err(error) if strict => return Err(error),
                Err(_) => empty_manifest(),
            }
        }
    };
    Ok(LoadedManifest {
        path,
        snapshot,
        manifest,
    })
}

pub(crate) fn save(root: &Path, loaded: LoadedManifest) -> Result<Option<AppliedWrite>> {
    let encoded = encode(&loaded.manifest)?;
    if loaded.snapshot.content() == Some(encoded.as_slice()) {
        return Ok(None);
    }
    Ok(Some(loaded.snapshot.replace_beneath(
        root,
        &loaded.path,
        &encoded,
    )?))
}

pub(crate) fn token(language: &str, attestation: &FormalAttestation) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(format!(
        "mathdoc-formal-attestation-v{EVIDENCE_SCHEME_VERSION}\0"
    ));
    digest.update(language.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(attestation)?);
    Ok(format!("{:x}", digest.finalize()))
}

fn empty_manifest() -> FormalAttestationManifest {
    FormalAttestationManifest {
        version: MANIFEST_VERSION,
        evidence_scheme_version: EVIDENCE_SCHEME_VERSION,
        nodes: BTreeMap::new(),
    }
}

fn encode(manifest: &FormalAttestationManifest) -> Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec_pretty(manifest)?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn validate_manifest(manifest: &FormalAttestationManifest, path: &Path) -> Result<()> {
    if manifest.version != MANIFEST_VERSION {
        bail!(
            "unsupported formal attestation manifest version {} in {}",
            manifest.version,
            path.display()
        );
    }
    if manifest.evidence_scheme_version != EVIDENCE_SCHEME_VERSION {
        bail!(
            "unsupported formal evidence scheme version {} in {}",
            manifest.evidence_scheme_version,
            path.display()
        );
    }
    for (fnode, languages) in &manifest.nodes {
        for (language, attestation) in [
            ("lean", languages.lean.as_ref()),
            ("rocq", languages.rocq.as_ref()),
        ] {
            let Some(attestation) = attestation else {
                continue;
            };
            if attestation.fnode != *fnode {
                bail!(
                    "formal attestation key does not match its fnode in {}",
                    path.display()
                );
            }
            for (field, digest) in [
                ("source", &attestation.source_sha256),
                ("artifact", &attestation.artifact_sha256),
                ("environment", &attestation.environment_sha256),
                ("compiler", &attestation.compiler_sha256),
            ] {
                validate_digest(field, language, digest, path)?;
            }
            if !Path::new(&attestation.compiler_path).is_absolute() {
                bail!(
                    "compiler path for {language} attestation is not absolute in {}",
                    path.display()
                );
            }
            for dependency_token in attestation.dependencies.values() {
                validate_digest("dependency", language, dependency_token, path)?;
            }
            for (dependency_path, digest) in &attestation.external_dependencies {
                if !Path::new(dependency_path).is_absolute() {
                    bail!(
                        "external dependency path for {language} attestation is not absolute in {}",
                        path.display()
                    );
                }
                validate_digest("external dependency", language, digest, path)?;
            }
        }
    }
    Ok(())
}

fn legacy_evidence_scheme_version() -> u32 {
    1
}

fn validate_digest(field: &str, language: &str, digest: &str, path: &Path) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!(
            "invalid {field} digest for {language} attestation in {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load, load_for_status, FormalAttestationManifest};

    #[test]
    fn empty_node_entries_are_not_attestations() {
        let manifest: FormalAttestationManifest =
            serde_json::from_str(r#"{"version":1,"nodes":{"node":{}}}"#).unwrap();

        assert_eq!(manifest.evidence_scheme_version, 1);
        assert!(!manifest.has_attestations());
        assert!(!manifest.has_attestation_for("node"));
    }

    #[test]
    fn unsupported_evidence_schemes_are_strict_or_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        std::fs::write(
            root.join(".mdc/formal-attestations.json"),
            b"{\"version\":1,\"evidence_scheme_version\":2,\"nodes\":{}}\n",
        )
        .unwrap();

        assert!(load(&root).is_err());
        assert!(!load_for_status(&root).unwrap().manifest.has_attestations());
    }
}
