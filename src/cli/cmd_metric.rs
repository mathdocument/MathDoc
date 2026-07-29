use anyhow::Result;

use crate::indcache::IndCache;

use super::{cwd, require_mdcroot};

pub(super) fn cmd_metric_ior(source: String) -> Result<i32> {
    let _profile = crate::profile::scope("cli::cmd_metric_ior");
    let mut cache = IndCache::open(require_mdcroot()?)?;
    cache.refresh_all()?;
    let (fnode, _, _) = cache.resolve_ref(&source, Some(&cwd()))?;
    let value =
        crate::metric::evaluate_node_metric(crate::metric::NodeMetricKind::Ior, &cache, &fnode)?;
    println!("{value}");
    Ok(0)
}
