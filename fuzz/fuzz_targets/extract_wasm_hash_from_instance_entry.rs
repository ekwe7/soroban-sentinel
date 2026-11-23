#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = sentinel_core::extract_wasm_hash_from_instance_entry(&input);
});
