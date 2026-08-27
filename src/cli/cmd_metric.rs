use anyhow::Result;

use crate::indcache::IndCache;

use super::{cwd, require_mdcroot};

pub(super) fn cmd_metric_ior(source: String) -> Result<i32> {
    let _profile = crate::profile::scope("cli::cmd_metric_ior");
    let cache = IndCache::open_refreshed(require_mdcroot()?)?;
    let (fnode, _, _) = cache.resolve_ref(&source, Some(&cwd()))?;
    let degrees = cache.node_degrees(&fnode)?;
    let value = f64::from(degrees.in_degree).ln_1p() - f64::from(degrees.out_degree).ln_1p();
    println!("{value}");
    Ok(0)
}
