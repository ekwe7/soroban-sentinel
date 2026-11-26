use assert_cmd::Command;
use predicates::prelude::*;

fn make_valid_manifest(path: &str) {
    let manifest = serde_json::json!({
        "network": "testnet",
        "contracts": [
            {
                "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                "warning_threshold": 100000,
                "critical_threshold": 30000
            },
            {
                "contract_id": "CA3D5NXQ6DNG5BB25US67P3J6R74V4A7N5E55LFEV74J5R4R37E2L6G5",
                "warning_threshold": 200000,
                "critical_threshold": 50000
            }
        ],
        "warning_threshold": 120960,
        "critical_threshold": 34560
    });

    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
}

fn make_manifest_with_invalid_contract(path: &str) {
    let manifest = serde_json::json!({
        "network": "testnet",
        "contracts": [
            {
                "contract_id": "INVALID_CONTRACT_ID_NOT_A_STRKEY"
            }
        ],
        "warning_threshold": 120960,
        "critical_threshold": 34560
    });

    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
}

fn make_manifest_with_warning_le_critical(path: &str) {
    let manifest = serde_json::json!({
        "network": "testnet",
        "contracts": [
            {
                "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                "warning_threshold": 10000,
                "critical_threshold": 20000
            }
        ],
        "warning_threshold": 10000,
        "critical_threshold": 20000
    });

    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
}

fn make_manifest_with_duplicate_contracts(path: &str) {
    let manifest = serde_json::json!({
        "network": "testnet",
        "contracts": [
            {
                "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                "warning_threshold": 100000,
                "critical_threshold": 30000
            },
            {
                "contract_id": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                "warning_threshold": 200000,
                "critical_threshold": 50000
            }
        ],
        "warning_threshold": 120960,
        "critical_threshold": 34560
    });

    std::fs::write(path, serde_json::to_string(&manifest).unwrap()).unwrap();
}

#[test]
fn test_lint_manifest_valid() {
    let temp_file = std::env::temp_dir().join("valid_manifest_test.json");
    make_valid_manifest(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["lint-manifest", "--manifest", temp_file.to_str().unwrap()]);

    cmd.assert()
        .code(0)
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains(
            "Manifest '/tmp/valid_manifest_test.json' validated successfully",
        ));
}

#[test]
fn test_lint_manifest_invalid_contract_id() {
    let temp_file = std::env::temp_dir().join("invalid_contract_test.json");
    make_manifest_with_invalid_contract(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["lint-manifest", "--manifest", temp_file.to_str().unwrap()]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid contract ID"));
}

#[test]
fn test_lint_manifest_warning_le_critical() {
    let temp_file = std::env::temp_dir().join("warning_le_critical_test.json");
    make_manifest_with_warning_le_critical(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["lint-manifest", "--manifest", temp_file.to_str().unwrap()]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("warning_threshold"));
}

#[test]
fn test_lint_manifest_with_duplicate_contracts() {
    let temp_file = std::env::temp_dir().join("duplicate_contracts_test.json");
    make_manifest_with_duplicate_contracts(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["lint-manifest", "--manifest", temp_file.to_str().unwrap()]);

    cmd.assert()
        .code(0)
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains(
            "Manifest '/tmp/duplicate_contracts_test.json' validated successfully",
        ));
}

#[test]
fn test_schema_command_generates_json() {
    let temp_file = std::env::temp_dir().join("schema_test.json");
    make_valid_manifest(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["schema", "--manifest", temp_file.to_str().unwrap()]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let _parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Schema output must be valid JSON");
}

#[test]
fn test_schema_command_has_required_fields() {
    let temp_file = std::env::temp_dir().join("schema_fields_test.json");
    make_valid_manifest(temp_file.to_str().unwrap());

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["schema", "--manifest", temp_file.to_str().unwrap()]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Schema output must be valid JSON");

    // Schema should have title, type, properties
    assert!(parsed["title"].is_string(), "Schema must have a title");
    assert!(parsed["type"].is_string(), "Schema must have a type");
    assert!(
        parsed["properties"].is_object(),
        "Schema must have properties"
    );
}
