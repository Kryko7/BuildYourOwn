//! `byo` — the Build Your Own toolkit's library half: project config, the progress
//! database, the tester runner, the JSON API and the static site server.
//!
//! The `byo` binary is a thin CLI over these modules; the integration tests in `tests/`
//! drive them directly.

#![warn(missing_docs)]

pub mod api;
pub mod branding;
pub mod catalog;
pub mod config;
pub mod db;
pub mod doctor;
pub mod paths;
pub mod report;
pub mod runner;
pub mod server;
pub mod static_files;
pub mod status;
pub mod track;
