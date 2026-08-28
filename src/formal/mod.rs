use std::collections::BTreeMap;

pub(crate) mod attestation;
pub(crate) mod status;

/// Bump when any formal evidence digest, token, or validation recipe changes.
pub(crate) const EVIDENCE_SCHEME_VERSION: u32 = 1;
pub(crate) const ROCQ_CLEAN_MARKER_FILENAME: &str = ".mdc-clean-needed";

#[derive(Clone, Debug)]
pub(crate) struct FormalCompilationReceipt {
    pub(crate) evidence_scheme_version: u32,
    pub(crate) language: String,
    pub(crate) target_module: String,
    pub(crate) source_sha256: String,
    pub(crate) artifact_sha256: String,
    pub(crate) environment_sha256: String,
    pub(crate) compiler_path: String,
    pub(crate) compiler_sha256: String,
    /// Direct workspace module key to the artifact digest consumed by the build.
    pub(crate) direct_dependencies: BTreeMap<String, String>,
    /// Canonical external artifact path to the digest consumed by the build.
    pub(crate) external_dependencies: BTreeMap<String, String>,
}
