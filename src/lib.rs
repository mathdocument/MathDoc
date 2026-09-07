#[cfg(not(unix))]
compile_error!("mathdoc currently supports Unix platforms only");

#[cfg(unix)]
#[path = "cli_v2.rs"]
mod cli;
#[cfg(unix)]
pub use cli::run;
#[cfg(unix)]
pub mod config;
#[cfg(unix)]
pub mod core;
pub mod store;
pub mod service;
pub mod lean;
#[cfg(unix)]
pub mod mdocnode;
pub(crate) mod profile;
#[cfg(unix)]
pub mod web;
