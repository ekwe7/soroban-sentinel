use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{args::RedactArgs, commands::EXIT_INVALID_ARGS};

pub fn run_redact(args: RedactArgs) -> i32 {
    let source = match fs::read(&args.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("Error: cannot read input report: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let mut report: Value = match serde_json::from_slice(&source) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error: input is not valid JSON: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    redact_value(&mut report, args.salt.as_deref());
    let output = match serde_json::to_vec_pretty(&report) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("Error: cannot serialize redacted report: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    if let Some(path) = args.output {
        if let Err(error) = write_output(&path, &output) {
            eprintln!("Error: cannot write output report: {error}");
            return EXIT_INVALID_ARGS;
        }
    } else if let Err(error) = io::stdout()
        .write_all(&output)
        .and_then(|_| io::stdout().write_all(b"\n"))
    {
        eprintln!("Error: cannot write redacted report: {error}");
        return EXIT_INVALID_ARGS;
    }
    0
}

fn write_output(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

fn redact_value(value: &mut Value, salt: Option<&str>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if key.eq_ignore_ascii_case("contract_id") {
                    if let Some(text) = child.as_str() {
                        *child = Value::String(mask_contract_id(text));
                    }
                } else if is_sensitive_key(key) {
                    let encoded = serde_json::to_vec(child).unwrap_or_default();
                    *child = Value::String(pseudonym(&encoded, salt));
                } else {
                    redact_value(child, salt);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_value(item, salt);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "key" | "storage_key" | "key_summary" | "identifier" | "payload" | "value_xdr"
    ) || key.ends_with("_xdr")
        || key.ends_with("_xdr_base64")
        || key.ends_with("_payload")
}

fn mask_contract_id(value: &str) -> String {
    if value.chars().count() <= 8 {
        return "***".into();
    }
    let prefix: String = value.chars().take(4).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn pseudonym(value: &[u8], salt: Option<&str>) -> String {
    let Some(salt) = salt.filter(|salt| !salt.is_empty()) else {
        return "***".into();
    };
    // RFC 2104 HMAC-SHA-256 using SHA-256's 64-byte block size.
    let mut key = salt.as_bytes().to_vec();
    if key.len() > 64 {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(64, 0);
    let mut inner_pad = vec![0x36; 64];
    let mut outer_pad = vec![0x5c; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(value);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    format!("***:{}", hex(&outer.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
