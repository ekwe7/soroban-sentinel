//! Command-line interface library for Soroban Sentinel.

pub mod args;
pub mod formatters;

pub use args::{Cli, Commands, ScanArgs};
pub use formatters::{render_json, render_text, JsonEntryReport, JsonSummary, JsonTTLReport};
