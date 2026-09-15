//! Project-scoped LaTeX configuration, reference resolution and HTML previews.
mod api;
mod project;

pub(crate) use api::routes;
pub use project::LatexProject;
