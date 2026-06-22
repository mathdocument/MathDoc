use anyhow::Result;

use super::{cwd, open_cache, require_mdcroot};
use crate::indcache::IndCache;
use crate::web;

/// `mdc serve` — start the interactive web frontend.
pub(super) fn cmd_serve(source: Option<String>, bind: String, no_open: bool) -> Result<i32> {
    let mdcroot = require_mdcroot()?;
    let mut cache: IndCache = open_cache(mdcroot.clone())?;
    cache.discover_workspace_changes()?;

    // If the caller gave us a starting ref, validate it now so we can fail
    // fast with a clear CLI error instead of a 400 from the browser.
    if let Some(ref s) = source {
        match cache.resolve_ref(s, Some(&cwd())) {
            Ok((fnode, _, _)) => {
                eprintln!("starting at: {} ({})", &fnode[..fnode.len().min(8)], s);
            }
            Err(e) => anyhow::bail!("cannot resolve '{}': {}", s, e),
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(web::server::serve(mdcroot, cache, &bind, !no_open))?;
    Ok(0)
}
