mod function;

use anyhow::{bail, Result};

use crate::core::NodeDegrees;
use crate::indcache::IndCache;

struct MetricContext<'cache> {
    cache: &'cache IndCache,
}

impl<'cache> MetricContext<'cache> {
    fn new(cache: &'cache IndCache) -> Self {
        Self { cache }
    }

    fn node_degrees(&self, fnode: &str) -> Result<NodeDegrees> {
        self.cache.node_degrees(fnode)
    }
}

trait NodeMetric: Sync {
    fn evaluate(&self, context: &MetricContext<'_>, fnode: &str) -> Result<f64>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeMetricKind {
    Ior,
}

impl NodeMetricKind {
    fn name(self) -> &'static str {
        match self {
            Self::Ior => "ior",
        }
    }

    fn implementation(self) -> &'static dyn NodeMetric {
        match self {
            Self::Ior => &IOR_METRIC,
        }
    }
}

struct IorMetric;

impl NodeMetric for IorMetric {
    fn evaluate(&self, context: &MetricContext<'_>, fnode: &str) -> Result<f64> {
        Ok(function::ior(context.node_degrees(fnode)?))
    }
}

static IOR_METRIC: IorMetric = IorMetric;

pub(crate) fn evaluate_node_metric(
    kind: NodeMetricKind,
    cache: &IndCache,
    fnode: &str,
) -> Result<f64> {
    let metric = kind.implementation();
    let value = metric.evaluate(&MetricContext::new(cache), fnode)?;
    if !value.is_finite() {
        bail!("metric '{}' produced a non-finite value", kind.name());
    }
    Ok(value)
}
