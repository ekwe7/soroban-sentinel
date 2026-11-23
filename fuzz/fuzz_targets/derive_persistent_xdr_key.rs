#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = sentinel_core::derive_persistent_xdr_key(&[7u8; 32], &input);
});
