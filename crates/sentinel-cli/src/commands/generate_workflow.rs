//! Execution logic for the `sentinel generate-github-workflow` CLI command.
//!
//! Scaffolds a production-ready GitHub Actions workflow for automated Soroban storage TTL
//! health audits, scheduled drift scanning, step summary reporting, and critical threshold enforcement.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::GenerateWorkflowArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};

pub const DEFAULT_CRON_SCHEDULE: &str = "0 0 * * *";
pub const WORKFLOW_FILE_NAME: &str = "sentinel-ttl-audit.yml";

fn validate_workflow_inputs(cron_schedule: &str, network: &str) -> Result<(), String> {
    let fields: Vec<_> = cron_schedule.split_ascii_whitespace().collect();
    if cron_schedule.contains('\n')
        || cron_schedule.contains('\r')
        || fields.len() != 5
        || fields.iter().any(|field| {
            field.is_empty()
                || !field
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || "*,-/".contains(ch))
        })
    {
        return Err("cron must be a five-field GitHub Actions schedule".to_string());
    }

    if !matches!(network, "testnet" | "mainnet" | "futurenet" | "local") {
        return Err("network must be testnet, mainnet, futurenet, or local".to_string());
    }

    Ok(())
}

/// Generates valid YAML workflow content with the specified cron schedule and network.
pub fn generate_workflow_yaml(cron_schedule: &str, network: &str) -> Result<String, String> {
    validate_workflow_inputs(cron_schedule, network)?;
    Ok(format!(
        r#"name: Sentinel TTL Audit

on:
  schedule:
    - cron: '{cron_schedule}'
  workflow_dispatch:

jobs:
  ttl-audit:
    name: Soroban Storage TTL Audit
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Install soroban-sentinel
        run: cargo install soroban-sentinel --locked

      - name: Run Sentinel TTL Scan and Enforce Critical Risk
        env:
          SENTINEL_CONTRACT_ID: ${{{{ vars.SENTINEL_CONTRACT_ID }}}}
          SOROBAN_RPC_URL: ${{{{ secrets.SOROBAN_RPC_URL }}}}
        run: |
          set -euo pipefail
          : "${{SENTINEL_CONTRACT_ID:?Set repository variable SENTINEL_CONTRACT_ID}}"
          sentinel scan "$SENTINEL_CONTRACT_ID" --network {network} --format markdown --fail-on-risk critical | tee -a "$GITHUB_STEP_SUMMARY"
"#
    ))
}

/// Resolves the target destination file path from `--out-dir`.
pub fn resolve_destination_path(out_dir: &Path) -> PathBuf {
    if out_dir.ends_with(".github/workflows") || out_dir.ends_with(".github/workflows/") {
        out_dir.join(WORKFLOW_FILE_NAME)
    } else {
        out_dir
            .join(".github")
            .join("workflows")
            .join(WORKFLOW_FILE_NAME)
    }
}

pub fn run_generate_workflow(args: GenerateWorkflowArgs) -> i32 {
    let cron_schedule = args.cron.trim();
    if let Err(error) = validate_workflow_inputs(cron_schedule, &args.network) {
        eprintln!("Error: {error}");
        return EXIT_INVALID_ARGS;
    }

    let target_path = resolve_destination_path(&args.out_dir);

    if target_path.exists() && !args.force {
        eprintln!(
            "Error: workflow '{}' already exists; pass --force to replace it",
            target_path.display()
        );
        return EXIT_INVALID_ARGS;
    }

    if let Some(parent) = target_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!(
                "Error: failed to create directories '{}': {}",
                parent.display(),
                e
            );
            return EXIT_INVALID_ARGS;
        }
    }

    let yaml_content = match generate_workflow_yaml(cron_schedule, &args.network) {
        Ok(content) => content,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    let mut options = OpenOptions::new();
    options.write(true);
    if args.force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = match options.open(&target_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "Error: workflow '{}' already exists; pass --force to replace it",
                target_path.display()
            );
            return EXIT_INVALID_ARGS;
        }
        Err(e) => {
            eprintln!(
                "Error: failed to open '{}' for writing: {}",
                target_path.display(),
                e
            );
            return EXIT_INVALID_ARGS;
        }
    };

    if let Err(e) = file.write_all(yaml_content.as_bytes()) {
        eprintln!(
            "Error: failed to write to '{}': {}",
            target_path.display(),
            e
        );
        return EXIT_INVALID_ARGS;
    }

    println!(
        "Successfully generated GitHub Actions workflow: {}",
        target_path.display()
    );
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_workflow_yaml_content() {
        let yaml = generate_workflow_yaml("30 2 * * *", "testnet").unwrap();
        assert!(yaml.contains("cron: '30 2 * * *'"));
        assert!(yaml.contains("workflow_dispatch:"));
        assert!(yaml.contains("dtolnay/rust-toolchain@stable"));
        assert!(yaml.contains("cargo install soroban-sentinel --locked"));
        assert!(yaml.contains("SENTINEL_CONTRACT_ID: ${{ vars.SENTINEL_CONTRACT_ID }}"));
        assert!(yaml.contains("sentinel scan \"$SENTINEL_CONTRACT_ID\" --network testnet --format markdown --fail-on-risk critical"));
        assert!(yaml.contains("| tee -a \"$GITHUB_STEP_SUMMARY\""));
    }

    #[test]
    fn test_rejects_invalid_schedule_and_network() {
        assert!(generate_workflow_yaml("0 0 * * *\n  inject: true", "mainnet").is_err());
        assert!(generate_workflow_yaml("0 0 * * *", "mainnet; echo bad").is_err());
    }

    #[test]
    fn test_resolve_destination_path() {
        let p1 = Path::new(".");
        assert_eq!(
            resolve_destination_path(p1),
            PathBuf::from("./.github/workflows/sentinel-ttl-audit.yml")
        );

        let p2 = Path::new("/workspace/.github/workflows");
        assert_eq!(
            resolve_destination_path(p2),
            PathBuf::from("/workspace/.github/workflows/sentinel-ttl-audit.yml")
        );
    }
}
