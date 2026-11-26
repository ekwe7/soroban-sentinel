use sentinel_core::adapters::{analyze_wasm, MAX_WASM_ANALYSIS_BYTES};
use sentinel_core::domain::NetworkRentParameters;
use sentinel_core::error::SentinelError;

fn wasm_module(data_len: usize) -> Vec<u8> {
    assert!(data_len <= u8::MAX as usize);
    let mut wasm = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];
    wasm.extend_from_slice(&[0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
    wasm.extend_from_slice(&[0x03, 0x02, 0x01, 0x00]);
    wasm.extend_from_slice(&[0x05, 0x03, 0x01, 0x00, 0x01]);
    wasm.extend_from_slice(&[0x09, 0x04, 0x01, 0x00, 0x01, 0x00]);
    wasm.extend_from_slice(&[
        0x07, 0x0a, 0x01, 0x06, b'm', b'e', b'm', b'o', b'r', b'y', 0x02, 0x00,
    ]);
    wasm.extend_from_slice(&[0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b]);
    wasm.extend_from_slice(&[
        0x00, 0x09, 0x04, b'm', b'e', b't', b'a', 0x01, 0x02, 0x03, 0x04,
    ]);

    let data_payload_len = 6 + data_len;
    wasm.extend_from_slice(&[0x0b, data_payload_len as u8, 0x01, 0x00, 0x41, 0x00, 0x0b]);
    wasm.push(data_len as u8);
    wasm.extend(std::iter::repeat_n(0xab, data_len));
    wasm
}

#[test]
fn section_sizes_are_counted_once_and_sum_to_file_size() {
    let wasm = wasm_module(20);
    let analysis = analyze_wasm(&wasm, None).expect("valid WASM");

    assert_eq!(analysis.file_size, wasm.len());
    assert_eq!(analysis.sections.total(), wasm.len());
    assert_eq!(analysis.sections.code, 4);
    assert_eq!(analysis.sections.type_, 4);
    assert_eq!(analysis.sections.element, 4);
    assert_eq!(analysis.sections.export, 10);
    assert_eq!(analysis.sections.custom, 9);
    assert_eq!(analysis.sections.data, 26);
}

#[test]
fn embedded_data_rent_uses_existing_network_rent_formula() {
    let wasm = wasm_module(20);
    let rent = NetworkRentParameters::try_new(10.0, 2.0, 100, 1_000, 100, 6_000)
        .expect("valid rent parameters");
    let analysis = analyze_wasm(&wasm, Some(&rent)).expect("valid WASM");

    assert_eq!(
        analysis.embedded_data_rent_stroops,
        analysis.sections.data as f64 * 1_200.0
    );
}

#[test]
fn warning_fires_only_above_quarter_of_executable_size() {
    assert!(analyze_wasm(&wasm_module(2), None)
        .expect("valid WASM")
        .warning
        .is_none());
    assert!(analyze_wasm(&wasm_module(20), None)
        .expect("valid WASM")
        .warning
        .is_some());
}

#[test]
fn malformed_and_oversized_inputs_return_typed_errors() {
    let malformed = analyze_wasm(
        &[0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00, 0x01],
        None,
    );
    assert!(matches!(
        malformed,
        Err(SentinelError::WasmAnalysisError(_))
    ));

    let oversized = vec![0u8; MAX_WASM_ANALYSIS_BYTES + 1];
    let oversized = analyze_wasm(&oversized, None);
    assert!(matches!(
        oversized,
        Err(SentinelError::WasmAnalysisError(_))
    ));
}
