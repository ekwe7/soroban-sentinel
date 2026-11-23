#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = sentinel_core::adapters::fuzz_parse_get_ledger_entries_response(data);
});
