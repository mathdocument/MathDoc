#[cfg(not(unix))]
compile_error!("mathdoc currently supports Unix platforms only");

#[cfg(unix)]
mod cli;
#[cfg(unix)]
pub use cli::run;
#[cfg(unix)]
pub mod config;
#[cfg(unix)]
pub mod core;
pub mod lean;
pub(crate) mod profile;
pub mod service;
pub(crate) mod server;
pub mod store;
#[cfg(unix)]
pub mod web;
