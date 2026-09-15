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
pub mod latex;
pub mod lean;
pub(crate) mod profile;
pub(crate) mod server;
pub mod service;
pub mod store;
#[cfg(unix)]
pub mod web;
