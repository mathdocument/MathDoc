use anyhow::Result;

use super::{require_mdcroot, resolve_start_ref, short_fnode};
use crate::core::escape_terminal;
use crate::indcache::IndCache;
use crate::web;

/// `mdc serve` — start the interactive web frontend.
pub(super) fn cmd_serve(source: Option<String>, bind: String, no_open: bool) -> Result<i32> {
    let _profile = crate::profile::scope("cli::cmd_serve");
    let mut cache = IndCache::open(require_mdcroot()?)?;

    // If the caller gave us a starting ref, validate it now so we can fail
    // fast with a clear CLI error instead of a 400 from the browser.
    let initial_fnode = resolve_initial_fnode(&mut cache, source.as_deref())?;
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

fn resolve_initial_fnode(cache: &mut IndCache, source: Option<&str>) -> Result<Option<String>> {
    let Some(source) = source else {
        return Ok(None);
    };
    cache.discover_workspace_changes()?;
    Ok(Some(resolve_start_ref(cache, source)?))
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
            resolve_initial_fnode(&mut cache, Some("Start Here")).unwrap(),
            Some("start-node".to_string())
        );
    }

    #[test]
    fn serve_without_start_ref_defers_workspace_discovery() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".mdc")).unwrap();
        let mut cache = IndCache::open(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.path().join("external.mdoc"),
            "@fnode: external-node\n@title: External\n",
        )
        .unwrap();

        assert_eq!(resolve_initial_fnode(&mut cache, None).unwrap(), None);
        assert!(cache.search("External", 10).unwrap().is_empty());
        assert_eq!(
            resolve_initial_fnode(&mut cache, Some("External")).unwrap(),
            Some("external-node".to_string())
        );
    }
}
