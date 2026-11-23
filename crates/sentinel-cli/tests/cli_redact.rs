use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_file(suffix: &str) -> std::path::PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sentinel-redact-{}-{id}-{suffix}.json",
        std::process::id()
    ))
}

#[test]
fn redacts_secrets_and_preserves_ttl_risk_metadata_with_stable_hmac() {
    let input = temp_file("input");
    let output = temp_file("output");
    let source = r#"{"contract_id":"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM","entries":[{"identifier":"persistent:PrivateBalance","key_xdr_base64":"c2VjcmV0LWtleS14ZHI=","entry_xdr_base64":"c2VjcmV0LXZhbHVl","durability":"persistent","remaining_ledgers":123,"status":"critical"}] }"#;
    fs::write(&input, source).unwrap();
    let status = Command::new(assert_cmd::cargo::cargo_bin("sentinel"))
        .args([
            "redact",
            "--input",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--salt",
            "local-test-secret",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let output_bytes = fs::read(&output).unwrap();
    let output_text = String::from_utf8(output_bytes.clone()).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();
    assert!(!output_text.contains("PrivateBalance"));
    assert!(!output_text.contains("c2VjcmV0"));
    assert_eq!(report["contract_id"], "CAAA...D2KM");
    assert_eq!(report["entries"][0]["durability"], "persistent");
    assert_eq!(report["entries"][0]["remaining_ledgers"], 123);
    assert_eq!(report["entries"][0]["status"], "critical");
    assert!(report["entries"][0]["key_xdr_base64"]
        .as_str()
        .unwrap()
        .starts_with("***:"));
    let second_output = temp_file("second-output");
    let second = Command::new(assert_cmd::cargo::cargo_bin("sentinel"))
        .args([
            "redact",
            "--input",
            input.to_str().unwrap(),
            "--output",
            second_output.to_str().unwrap(),
            "--salt",
            "local-test-secret",
        ])
        .status()
        .unwrap();
    assert!(second.success());
    assert_eq!(
        fs::read(&output).unwrap(),
        fs::read(&second_output).unwrap()
    );
    fs::remove_file(input).unwrap();
    fs::remove_file(output).unwrap();
    fs::remove_file(second_output).unwrap();
}

#[test]
fn redacts_without_salt_and_emits_parseable_json_to_stdout() {
    let input = temp_file("input");
    fs::write(&input, r#"{"key":"private","remaining_ledgers":7}"#).unwrap();
    let output = Command::new(assert_cmd::cargo::cargo_bin("sentinel"))
        .args(["redact", "--input", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["key"], "***");
    assert_eq!(value["remaining_ledgers"], 7);
    fs::remove_file(input).unwrap();
}
