//! Output formatters for scan reports.

pub mod json;
pub mod sarif;
pub mod text;

pub use json::{render_json, render_json_with_diff, JsonEntryReport, JsonSummary, JsonTTLReport};
pub use sarif::render_sarif;
pub use text::{format_with_commas, render_text, render_text_with_diff};
