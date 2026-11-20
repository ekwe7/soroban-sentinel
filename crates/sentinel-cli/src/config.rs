//! Manifest configuration loader and validator for sentinel lint-manifest.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuration structure for a sentinel manifest file.
///
/// A manifest file defines a fleet of Soroban contracts to monitor,
/// along with threshold settings for risk classification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManifestConfig {
    /// Target network (testnet, mainnet, futurenet, local)
    pub network: String,

    /// List of contract IDs to scan, with associated thresholds
    #[serde(default)]
    pub contracts: Vec<ManifestContractConfig>,

    /// Global warning threshold in ledgers (default: 120960 ≈ 7 days)
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold: u32,

    /// Global critical threshold in ledgers (default: 34560 ≈ 2 days)
    #[serde(default = "default_critical_threshold")]
    pub critical_threshold: u32,
}

impl ManifestConfig {
    /// Load and validate a manifest from the given file path.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Err(format!("Manifest file not found: {}", path.display()));
        }

        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read manifest file '{}': {}", path.display(), e))?;

        let config: Self = serde_json::from_str(&contents)
            .map_err(|e| format!("Malformed manifest JSON in '{}': {}", path.display(), e))?;

        config.validate()?;

        Ok(config)
    }

    /// Validate the manifest configuration.
    fn validate(&self) -> Result<(), String> {
        // Validate network name
        match self.network.as_str() {
            "testnet" | "mainnet" | "futurenet" | "local" => {}
            other => {
                return Err(format!(
                    "Invalid network '{}'. Use: testnet, mainnet, futurenet, or local",
                    other
                ))
            }
        }

        // Validate contract IDs and ensure warning_threshold > critical_threshold
        for contract in &self.contracts {
            // Validate contract Strkey format (must start with C for contracts or G for accounts)
            let contract_id = &contract.contract_id;
            if contract_id.is_empty()
                || (!contract_id.starts_with('C')
                    && !contract_id.starts_with('G')
                    && !contract_id.starts_with('S'))
            {
                return Err(format!(
                    "Invalid contract ID '{}'. Strkey must start with 'C' (contract), 'G' (account), or 'S' (seed)",
                    contract_id
                ));
            }

            // Ensure warning_threshold is strictly greater than critical_threshold
            if self.warning_threshold <= self.critical_threshold {
                return Err(format!(
                    "warning_threshold ({}) must be strictly greater than critical_threshold ({})",
                    self.warning_threshold, self.critical_threshold
                ));
            }

            // Validate contract-specific thresholds if provided
            if let Some(warn) = contract.warning_threshold {
                if warn <= self.critical_threshold {
                    return Err(format!(
                        "contract '{}' warning_threshold ({}) must be strictly greater than global critical_threshold ({})",
                        contract.contract_id, warn, self.critical_threshold
                    ));
                }
            }

            if let Some(crit) = contract.critical_threshold {
                if self.warning_threshold <= crit {
                    return Err(format!(
                        "contract '{}' critical_threshold ({}) must be strictly less than global warning_threshold ({})",
                        contract.contract_id, crit, self.warning_threshold
                    ));
                }
            }
        }

        Ok(())
    }
}

fn default_warning_threshold() -> u32 {
    120960
}

fn default_critical_threshold() -> u32 {
    34560
}

/// Configuration for a single contract within a manifest.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManifestContractConfig {
    /// Target Soroban contract address (C... Strkey format)
    pub contract_id: String,

    /// Per-contract warning threshold (overrides global if set)
    #[serde(default)]
    pub warning_threshold: Option<u32>,

    /// Per-contract critical threshold (overrides global if set)
    #[serde(default)]
    pub critical_threshold: Option<u32>,
}
