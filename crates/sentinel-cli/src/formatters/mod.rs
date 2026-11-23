//! Output formatters for scan reports.

pub mod json;
pub mod markdown;
#[cfg(feature = "parquet")]
pub mod parquet;
pub mod sarif;
pub mod sonarqube;
pub mod tabular;
pub mod text;

pub use json::{render_json, render_json_with_diff, JsonEntryReport, JsonSummary, JsonTTLReport};
pub use markdown::render_markdown;
#[cfg(feature = "parquet")]
pub use parquet::render_parquet;
pub use sarif::render_sarif;
pub use sonarqube::render_sonarqube;
pub use tabular::{build_tabular_rows, render_csv, TabularRow, CSV_HEADER};
pub use text::{format_with_commas, render_text, render_text_with_diff};
