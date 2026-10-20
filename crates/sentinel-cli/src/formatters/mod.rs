//! Output formatters for scan reports.

pub mod json;
pub mod text;

pub use json::{render_json, JsonEntryReport, JsonSummary, JsonTTLReport};
pub use text::{format_with_commas, render_text};
