//! WASM binary section adapter using wasmparser.

use wasmparser::{Parser, Payload};

use crate::domain::scenario::NetworkRentParameters;
use crate::domain::wasm_analysis::{WasmAnalysis, WasmSectionSizes};
use crate::error::{SentinelError, SentinelResult};

/// Maximum WASM module size accepted for section analysis (16 MiB).
pub const MAX_WASM_ANALYSIS_BYTES: usize = 16 * 1024 * 1024;

/// Parses a WASM module and reports section categories.
///
/// Section ranges are counted once; the `CodeSectionStart` range already
/// includes its function bodies. Custom sections are counted by their encoded
/// range without interpreting their payload, so compressed custom data is
/// handled the same way as uncompressed data.
pub fn analyze_wasm(
    wasm: &[u8],
    rent: Option<&NetworkRentParameters>,
) -> SentinelResult<WasmAnalysis> {
    if wasm.len() > MAX_WASM_ANALYSIS_BYTES {
        return Err(SentinelError::WasmAnalysisError(
            "WASM input exceeds 16 MiB analysis limit".to_string(),
        ));
    }

    let mut sizes = WasmSectionSizes::default();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.map_err(|error| {
            SentinelError::WasmAnalysisError(format!("Malformed or truncated WASM: {error}"))
        })?;
        match payload {
            Payload::CodeSectionStart { range, .. } => {
                sizes.code = sizes.code.saturating_add(range.len());
            }
            Payload::DataSection(reader) => {
                sizes.data = sizes.data.saturating_add(reader.range().len());
            }
            Payload::ElementSection(reader) => {
                sizes.element = sizes.element.saturating_add(reader.range().len());
            }
            Payload::TypeSection(reader) => {
                sizes.type_ = sizes.type_.saturating_add(reader.range().len());
            }
            Payload::ExportSection(reader) => {
                sizes.export = sizes.export.saturating_add(reader.range().len());
            }
            Payload::CustomSection(reader) => {
                sizes.custom = sizes.custom.saturating_add(reader.range().len());
            }
            _ => {}
        }
    }

    let categorized = sizes
        .code
        .saturating_add(sizes.data)
        .saturating_add(sizes.element)
        .saturating_add(sizes.type_)
        .saturating_add(sizes.export)
        .saturating_add(sizes.custom);
    sizes.other = wasm.len().saturating_sub(categorized);

    Ok(crate::domain::wasm_analysis::analyze_wasm_sections(
        wasm.len(),
        sizes,
        rent,
    ))
}
