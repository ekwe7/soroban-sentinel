//! Pure WASM section analysis models.
//!
//! All sizes are byte counts. `other` absorbs the module header and every
//! section that is not categorized explicitly, so the category totals always
//! add up to the WASM file size.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WasmSectionSizes {
    pub code: usize,
    pub data: usize,
    pub element: usize,
    pub type_: usize,
    pub export: usize,
    pub custom: usize,
    pub other: usize,
}

impl WasmSectionSizes {
    pub fn total(&self) -> usize {
        self.code
            .saturating_add(self.data)
            .saturating_add(self.element)
            .saturating_add(self.type_)
            .saturating_add(self.export)
            .saturating_add(self.custom)
            .saturating_add(self.other)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WasmAnalysis {
    pub file_size: usize,
    pub sections: WasmSectionSizes,
    pub embedded_data_rent_stroops: f64,
    pub warning: Option<String>,
}

/// Derives the estimated monthly rent of embedded data sections from the
/// repository's existing rent formula:
/// `fee_rate_per_byte * rent_write_fee_multiplier * ledgers_per_month / minimum_persistent_ttl`
/// stroops per byte per month (see [`crate::domain::scenario::NetworkRentParameters`]).
///
/// A warning is emitted when data-section bytes exceed 25% of the non-data
/// (executable plus other) portion of the module.
pub fn analyze_wasm_sections(
    file_size: usize,
    sections: WasmSectionSizes,
    rent: Option<&crate::domain::scenario::NetworkRentParameters>,
) -> WasmAnalysis {
    let executable_size = sections
        .code
        .saturating_add(sections.element)
        .saturating_add(sections.type_)
        .saturating_add(sections.export)
        .saturating_add(sections.other);

    let embedded_data_rent_stroops = rent
        .map(|parameters| sections.data as f64 * parameters.monthly_cost_per_byte())
        .unwrap_or(0.0);

    let warning = if executable_size > 0 {
        let ratio = sections.data as f64 * 100.0 / executable_size as f64;
        (ratio > 25.0).then(|| {
            format!(
                "Embedded WASM data sections are {:.1}% of executable size ({} data bytes vs {} executable bytes); move lookup tables or constants to persistent storage where possible",
                ratio,
                sections.data,
                executable_size
            )
        })
    } else if sections.data > 0 {
        Some("Embedded WASM data sections exist but executable-size attribution is zero; move static data to persistent storage where possible".to_string())
    } else {
        None
    };

    WasmAnalysis {
        file_size,
        sections,
        embedded_data_rent_stroops,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scenario::NetworkRentParameters;

    #[test]
    fn rent_uses_existing_monthly_cost_formula() {
        let rent = NetworkRentParameters::try_new(10.0, 2.0, 100, 1_000, 100, 6_000)
            .expect("valid rent parameters");
        let analysis = analyze_wasm_sections(
            100,
            WasmSectionSizes {
                data: 10,
                code: 90,
                ..Default::default()
            },
            Some(&rent),
        );
        assert_eq!(analysis.embedded_data_rent_stroops, 12_000.0);
    }

    #[test]
    fn element_bytes_count_toward_executable_size() {
        let at_boundary = analyze_wasm_sections(
            125,
            WasmSectionSizes {
                data: 25,
                element: 100,
                ..Default::default()
            },
            None,
        );
        assert!(at_boundary.warning.is_none());
        let above = analyze_wasm_sections(
            126,
            WasmSectionSizes {
                data: 26,
                element: 100,
                ..Default::default()
            },
            None,
        );
        assert!(above.warning.is_some());
    }

    #[test]
    fn custom_section_bytes_do_not_count_as_executable_size() {
        let analysis = analyze_wasm_sections(
            1_100,
            WasmSectionSizes {
                data: 10,
                custom: 1_000,
                code: 90,
                ..Default::default()
            },
            None,
        );
        assert!(analysis.warning.is_none());
    }

    #[test]
    fn threshold_is_strictly_above_25_percent() {
        let below = analyze_wasm_sections(
            121,
            WasmSectionSizes {
                code: 97,
                data: 24,
                ..Default::default()
            },
            None,
        );
        assert!(below.warning.is_none());
        let at = analyze_wasm_sections(
            125,
            WasmSectionSizes {
                code: 100,
                data: 25,
                ..Default::default()
            },
            None,
        );
        assert!(at.warning.is_none());
        let above = analyze_wasm_sections(
            121,
            WasmSectionSizes {
                code: 96,
                data: 25,
                ..Default::default()
            },
            None,
        );
        assert!(above.warning.is_some());
        assert_eq!(below.sections.total(), 121);
        assert_eq!(at.sections.total(), 125);
        assert_eq!(above.sections.total(), 121);
    }
}
