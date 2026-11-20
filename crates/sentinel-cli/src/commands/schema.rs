//! JSON Schema generator and manifest linter for sentinel.

use crate::config::ManifestConfig;
use schemars::schema_for;
use serde_json::to_string_pretty;

/// Generates a JSON Schema object from the manifest configuration.
pub fn generate_schema() -> Result<String, String> {
    let schema = schema_for!(ManifestConfig);
    let json = to_string_pretty(&schema)
        .map_err(|e| format!("Failed to serialize schema to JSON: {}", e))?;
    Ok(json)
}

/// Lints a manifest file, validates its contents, and prints results.
///
/// This function:
/// 1. Loads the manifest from the given path
/// 2. Validates contract ID strkey checksums
/// 3. Ensures warning_threshold > critical_threshold
/// 4. Prints validation errors/warnings to stderr
/// 5. Returns (exit_code, validation_summary)
pub fn lint_manifest(manifest_path: &str) -> (i32, String) {
    let path = std::path::Path::new(manifest_path);

    // Try to load the manifest
    let config = match ManifestConfig::load(path) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Error: {}", err);
            return (10, err);
        }
    };

    // If we got here, validation passed
    let mut summary = String::new();
    summary.push_str(&format!(
        "Manifest '{}' validated successfully:\n",
        path.display()
    ));
    summary.push_str(&format!("  Network: {}\n", config.network));
    summary.push_str(&format!("  Contracts: {}\n", config.contracts.len()));
    summary.push_str(&format!(
        "  Warning threshold: {}\n",
        config.warning_threshold
    ));
    summary.push_str(&format!(
        "  Critical threshold: {}\n",
        config.critical_threshold
    ));

    // Print contract-specific details
    for contract in &config.contracts {
        summary.push_str(&format!(
            "  Contract '{}': warning={}, critical={}\n",
            contract.contract_id,
            contract
                .warning_threshold
                .unwrap_or(config.warning_threshold),
            contract
                .critical_threshold
                .unwrap_or(config.critical_threshold),
        ));
    }

    (0, summary)
}

/// Entry point for the `sentinel schema --manifest <PATH>` command.
///
/// Generates and exports JSON Schema for the manifest configuration,
/// suitable for IDE autocompletion (VS Code / IntelliJ).
pub fn schema_command(manifest_path: &str) -> i32 {
    let _path = std::path::Path::new(manifest_path);

    // Try to generate schema
    let schema_json = match generate_schema() {
        Ok(s) => s,
        Err(err) => {
            eprintln!("Error generating schema: {}", err);
            return 10;
        }
    };

    // Print schema to stdout
    println!("{}", schema_json);

    0
}
