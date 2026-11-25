//! Output formatters for scan reports.

pub mod json;
pub mod markdown;
pub mod ndjson;
#[cfg(feature = "parquet")]
pub mod parquet;
pub mod sarif;
pub mod tabular;
pub mod text;

pub use json::{render_json, render_json_with_diff, JsonEntryReport, JsonSummary, JsonTTLReport};
pub use markdown::render_markdown;
pub use ndjson::{build_ndjson_entries, render_ndjson, NdjsonEntry};
#[cfg(feature = "parquet")]
pub use parquet::render_parquet;
pub use sarif::render_sarif;
pub use tabular::{build_tabular_rows, render_csv, TabularRow, CSV_HEADER};
pub use text::{
    format_with_commas, is_color_disabled, is_no_color_env, render_text, render_text_with_diff,
    render_text_with_diff_and_options, render_text_with_options,
};
