use anyhow::Result;

use super::{require_mdcroot, resolve_start_ref, short_fnode};
use crate::core::escape_terminal;
use crate::indcache::IndCache;
use crate::web;

/// `mdc serve` — start the interactive web frontend.
pub(super) fn cmd_serve(source: Option<String>, bind: String, no_open: bool) -> Result<i32> {
    let _profile = crate::profile::scope("cli::cmd_serve");
    let mut cache = IndCache::open(require_mdcroot()?)?;
    cache.discover_workspace_changes()?;

    // If the caller gave us a starting ref, validate it now so we can fail
    // fast with a clear CLI error instead of a 400 from the browser.
    let initial_fnode = source
        .as_deref()
        .map(|source| resolve_start_ref(&cache, source))
        .transpose()?;
    if let (Some(source), Some(fnode)) = (&source, &initial_fnode) {
        eprintln!(
            "starting at: {} ({})",
            short_fnode(&escape_terminal(fnode)),
            escape_terminal(source)
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(web::server::serve(
        cache,
        &bind,
        !no_open,
        initial_fnode.as_deref(),
    ))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_start_ref_accepts_unique_title() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".mdc")).unwrap();
        std::fs::write(
            dir.path().join("start.mdoc"),
            "@fnode: start-node\n@title: Start Here\n",
        )
        .unwrap();
        let mut cache = IndCache::open(dir.path().to_path_buf()).unwrap();
        cache.refresh_all().unwrap();

        assert_eq!(
            resolve_start_ref(&cache, "Start Here").unwrap(),
            "start-node"
        );
    }
}
