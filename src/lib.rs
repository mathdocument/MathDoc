#[cfg(not(unix))]
compile_error!("mathdoc currently supports Unix platforms only");

#[cfg(unix)]
pub mod application;
#[cfg(unix)]
mod cli;
#[cfg(unix)]
pub use cli::run;
#[cfg(unix)]
pub(crate) mod compiler;
#[cfg(unix)]
pub mod config;
#[cfg(unix)]
pub mod core;
#[cfg(unix)]
pub mod depgraph;
#[cfg(unix)]
pub(crate) mod formal;
#[cfg(unix)]
pub mod indcache;
#[cfg(unix)]
pub mod mdocnode;
pub(crate) mod profile;
#[cfg(unix)]
pub mod web;
#[cfg(unix)]
pub(crate) mod workdraft;
#[cfg(unix)]
pub mod workspace;
