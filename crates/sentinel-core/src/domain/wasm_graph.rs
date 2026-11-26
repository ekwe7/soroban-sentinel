//! Pure dependency graph between contract instances and WASM code entries.
//!
//! Contract instances and WASM hashes are modeled as nodes in a bipartite
//! adjacency list. Code rent is charged once per bytecode entry, while
//! instance rent is charged once per referencing contract instance, so a
//! liability report combines both for every code node.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A node in the instance/WASM dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WasmGraphNode {
    /// A contract code entry identified by its WASM hash.
    Code([u8; 32]),
    /// A contract instance identified by its contract ID.
    Instance(String),
}

/// Errors produced while updating a [`WasmDependencyGraph`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WasmGraphError {
    /// A contract instance must have a non-empty identifier.
    #[error("contract ID must not be empty")]
    EmptyContractId,
}

/// Cumulative rent liability attributed to one WASM bytecode entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmRentLiability {
    /// Hash of the WASM bytecode entry.
    pub wasm_hash: [u8; 32],
    /// Number of contract instances currently referencing the bytecode.
    pub referencing_instances: usize,
    /// Estimated monthly rent for the bytecode entry itself, in stroops.
    pub code_monthly_rent_stroops: u128,
    /// Sum of monthly rent across all referencing instances, in stroops.
    pub instances_monthly_rent_stroops: u128,
    /// Code rent plus all referencing instance rent, in stroops.
    pub cumulative_monthly_rent_stroops: u128,
}

/// Tracks which contract instances reference each WASM code hash.
///
/// Code nodes survive instance rebinding so that bytecode left behind by an
/// upgrade remains visible to [`WasmDependencyGraph::find_orphaned_wasm_hashes`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WasmDependencyGraph {
    adjacency: BTreeMap<WasmGraphNode, BTreeSet<WasmGraphNode>>,
    code_rent_stroops: BTreeMap<[u8; 32], u128>,
    instance_rent_stroops: BTreeMap<String, u128>,
    instance_hash: BTreeMap<String, [u8; 32]>,
}

impl WasmDependencyGraph {
    /// Creates an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a known bytecode entry and its estimated monthly code rent.
    /// Registering an existing hash replaces the recorded rent.
    pub fn register_code(&mut self, wasm_hash: [u8; 32], monthly_rent_stroops: u128) {
        self.adjacency
            .entry(WasmGraphNode::Code(wasm_hash))
            .or_default();
        self.code_rent_stroops
            .insert(wasm_hash, monthly_rent_stroops);
    }

    /// Links an instance to a WASM hash and records its estimated monthly
    /// instance rent. An existing link is replaced, which models an upgrade:
    /// the previous bytecode entry becomes a potential orphan.
    pub fn register_instance(
        &mut self,
        contract_id: impl Into<String>,
        wasm_hash: [u8; 32],
        monthly_rent_stroops: u128,
    ) -> Result<(), WasmGraphError> {
        let contract_id = contract_id.into().trim().to_owned();
        if contract_id.is_empty() {
            return Err(WasmGraphError::EmptyContractId);
        }

        self.unlink_instance(&contract_id);
        let instance_node = WasmGraphNode::Instance(contract_id.clone());
        let code_node = WasmGraphNode::Code(wasm_hash);
        self.adjacency.entry(code_node.clone()).or_default();
        self.adjacency
            .entry(instance_node.clone())
            .or_default()
            .insert(code_node.clone());
        if let Some(neighbors) = self.adjacency.get_mut(&code_node) {
            neighbors.insert(instance_node);
        }
        self.code_rent_stroops.entry(wasm_hash).or_default();
        self.instance_rent_stroops
            .insert(contract_id.clone(), monthly_rent_stroops);
        self.instance_hash.insert(contract_id, wasm_hash);
        Ok(())
    }

    /// Removes an instance and its edge. Returns true when a link existed.
    pub fn unlink_instance(&mut self, contract_id: &str) -> bool {
        let Some(wasm_hash) = self.instance_hash.remove(contract_id) else {
            return false;
        };
        self.instance_rent_stroops.remove(contract_id);
        let instance_node = WasmGraphNode::Instance(contract_id.to_string());
        let code_node = WasmGraphNode::Code(wasm_hash);
        if let Some(neighbors) = self.adjacency.get_mut(&code_node) {
            neighbors.remove(&instance_node);
        }
        self.adjacency.remove(&instance_node);
        true
    }

    /// Returns the WASM hash an instance currently references, if registered.
    pub fn wasm_hash_for_instance(&self, contract_id: &str) -> Option<[u8; 32]> {
        self.instance_hash.get(contract_id).copied()
    }

    /// Iterates adjacent nodes in deterministic order.
    pub fn neighbors(&self, node: &WasmGraphNode) -> impl Iterator<Item = &WasmGraphNode> {
        self.adjacency
            .get(node)
            .into_iter()
            .flat_map(|neighbors| neighbors.iter())
    }

    /// Iterates all known code hashes in ascending byte order.
    pub fn wasm_hashes(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        self.code_rent_stroops.keys().copied()
    }

    /// Iterates contract IDs referencing a hash in lexicographic order.
    pub fn referencing_contracts(&self, wasm_hash: &[u8; 32]) -> impl Iterator<Item = &str> {
        self.neighbors(&WasmGraphNode::Code(*wasm_hash))
            .filter_map(|node| match node {
                WasmGraphNode::Instance(contract_id) => Some(contract_id.as_str()),
                WasmGraphNode::Code(_) => None,
            })
    }

    /// Returns code entries with no referencing contract instances.
    pub fn find_orphaned_wasm_hashes(&self) -> Vec<[u8; 32]> {
        self.wasm_hashes()
            .filter(|hash| self.referencing_contracts(hash).next().is_none())
            .collect()
    }

    /// Returns the recorded monthly code rent for a hash, or zero if unknown.
    pub fn code_rent_for_hash(&self, wasm_hash: &[u8; 32]) -> u128 {
        self.code_rent_stroops
            .get(wasm_hash)
            .copied()
            .unwrap_or_default()
    }

    /// Sums monthly instance rent across every instance sharing a WASM hash.
    /// Addition saturates at [`u128::MAX`].
    pub fn aggregate_instance_rent_for_hash(&self, wasm_hash: &[u8; 32]) -> u128 {
        self.referencing_contracts(wasm_hash)
            .filter_map(|contract_id| self.instance_rent_stroops.get(contract_id))
            .fold(0u128, |total, rent| total.saturating_add(*rent))
    }

    /// Returns the full rent breakdown for one code entry.
    pub fn rent_liability_for(&self, wasm_hash: &[u8; 32]) -> Option<WasmRentLiability> {
        if !self.code_rent_stroops.contains_key(wasm_hash) {
            return None;
        }
        let code_rent = self.code_rent_for_hash(wasm_hash);
        let instance_rent = self.aggregate_instance_rent_for_hash(wasm_hash);
        Some(WasmRentLiability {
            wasm_hash: *wasm_hash,
            referencing_instances: self.referencing_contracts(wasm_hash).count(),
            code_monthly_rent_stroops: code_rent,
            instances_monthly_rent_stroops: instance_rent,
            cumulative_monthly_rent_stroops: code_rent.saturating_add(instance_rent),
        })
    }

    /// Returns a rent breakdown for every known code entry, ordered by hash.
    pub fn rent_liabilities(&self) -> Vec<WasmRentLiability> {
        self.wasm_hashes()
            .filter_map(|hash| self.rent_liability_for(&hash))
            .collect()
    }

    /// Sums cumulative rent across all code entries, saturating at [`u128::MAX`].
    pub fn total_rent_liability_stroops(&self) -> u128 {
        self.rent_liabilities()
            .iter()
            .fold(0u128, |total, liability| {
                total.saturating_add(liability.cumulative_monthly_rent_stroops)
            })
    }

    /// Number of known bytecode entries.
    pub fn code_count(&self) -> usize {
        self.code_rent_stroops.len()
    }

    /// Number of registered contract instances.
    pub fn instance_count(&self) -> usize {
        self.instance_hash.len()
    }

    /// Returns true when no code entries or instances are registered.
    pub fn is_empty(&self) -> bool {
        self.code_rent_stroops.is_empty() && self.instance_hash.is_empty()
    }
}
