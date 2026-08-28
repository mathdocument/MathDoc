mod codec;
mod node;

pub(crate) use node::{MdocHead, MdocIdentity};
pub use node::{MdocNode, SrcBlock};

pub(crate) fn content_revision(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("{:x}", Sha256::digest(content))
}

#[derive(Debug, thiserror::Error)]
#[error("node revision does not match current content")]
pub(crate) struct RevisionMismatch;
