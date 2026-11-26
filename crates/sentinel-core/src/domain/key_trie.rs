//! Persistent storage key namespace trie and prefix collision validator.
//!
//! Complex Soroban contracts partition storage keys into hierarchical namespaces
//! (e.g. `user:profile:<id>`, `user:vault:<id>`, `admin:config`). This module validates
//! namespace hierarchies, detects prefix collisions, and computes storage footprints
//! per prefix branch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::StorageEntry;

/// Aggregate storage statistics for a key namespace prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NamespaceStats {
    /// Namespace prefix queried.
    pub prefix: String,
    /// Total number of storage keys in this namespace branch.
    pub key_count: usize,
    /// Total storage footprint in bytes across all entries in this namespace branch.
    pub total_byte_size: usize,
    /// Number of sub-branches directly below this prefix.
    pub sub_branch_count: usize,
    /// Whether this prefix is itself a terminal storage entry.
    pub is_terminal: bool,
}

/// Represents a detected prefix collision where a key is both a terminal entry
/// and an ancestor prefix of deeper keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixCollision {
    /// The prefix path segment / namespace where collision occurs.
    pub prefix: String,
    /// Key identifier of the terminal entry at this node.
    pub terminal_key: String,
    /// Key identifiers of deeper descendant entries extending from this prefix.
    pub deeper_keys: Vec<String>,
}

/// Radix trie node representing a namespace segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrieNode {
    /// String representation of the edge component leading to this node.
    pub segment: String,
    /// Storage entry terminating at this node, if any.
    pub entry: Option<StorageEntry>,
    /// Child nodes keyed by their edge component.
    pub children: BTreeMap<String, TrieNode>,
}

impl TrieNode {
    /// Creates an empty trie node for the given edge component.
    pub fn new(segment: String) -> Self {
        Self {
            segment,
            entry: None,
            children: BTreeMap::new(),
        }
    }

    /// Returns whether a storage entry terminates at this node.
    pub fn is_terminal(&self) -> bool {
        self.entry.is_some()
    }
}

/// Trie indexing `StorageEntry` items by hierarchical namespace components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyNamespaceTrie {
    root: TrieNode,
}

impl Default for KeyNamespaceTrie {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyNamespaceTrie {
    /// Creates a new empty `KeyNamespaceTrie`.
    pub fn new() -> Self {
        Self {
            root: TrieNode::new("".to_string()),
        }
    }

    /// Helper to split a key identifier into path segments.
    ///
    /// Splits on standard namespace delimiters: `:` or `/`.
    pub fn parse_segments(key: &str) -> Vec<String> {
        if key.is_empty() {
            return Vec::new();
        }
        key.split([':', '/'])
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// Inserts a `StorageEntry` by parsing its `key_identifier` into segments.
    pub fn insert(&mut self, entry: StorageEntry) {
        let segments = Self::parse_segments(entry.key_identifier());
        self.insert_with_segments(&segments, entry);
    }

    /// Inserts a `StorageEntry` using an explicit slice of key components / segments.
    pub fn insert_with_segments<S: AsRef<str>>(&mut self, segments: &[S], entry: StorageEntry) {
        let mut curr = &mut self.root;
        for seg in segments {
            let segment = seg.as_ref();
            curr = curr
                .children
                .entry(segment.to_string())
                .or_insert_with(|| TrieNode::new(segment.to_string()));
        }
        curr.entry = Some(entry);
    }

    /// Looks up a storage entry by exact key identifier.
    pub fn get(&self, key: &str) -> Option<&StorageEntry> {
        let segments = Self::parse_segments(key);
        self.get_with_segments(&segments)
    }

    /// Looks up a storage entry by an explicit sequence of XDR key components.
    pub fn get_with_segments<S: AsRef<str>>(&self, segments: &[S]) -> Option<&StorageEntry> {
        let mut curr = &self.root;
        for segment in segments {
            curr = curr.children.get(segment.as_ref())?;
        }
        curr.entry.as_ref()
    }

    /// Computes aggregate storage byte size and total key count for a specified namespace prefix.
    pub fn aggregate_namespace_size(&self, prefix: &str) -> NamespaceStats {
        let segments = Self::parse_segments(prefix);
        let mut stats = self.aggregate_namespace_size_segments(&segments);
        stats.prefix = prefix.to_string();
        stats
    }

    /// Computes aggregate storage size for an explicit sequence of XDR key components.
    pub fn aggregate_namespace_size_segments<S: AsRef<str>>(
        &self,
        segments: &[S],
    ) -> NamespaceStats {
        let mut curr = &self.root;
        for segment in segments {
            match curr.children.get(segment.as_ref()) {
                Some(next) => curr = next,
                None => {
                    return NamespaceStats {
                        prefix: segments
                            .iter()
                            .map(|segment| segment.as_ref())
                            .collect::<Vec<_>>()
                            .join(":"),
                        key_count: 0,
                        total_byte_size: 0,
                        sub_branch_count: 0,
                        is_terminal: false,
                    };
                }
            }
        }

        let mut key_count = 0;
        let mut total_byte_size = 0;
        let sub_branch_count = curr.children.len();
        let is_terminal = curr.is_terminal();

        Self::collect_subtree_stats(curr, &mut key_count, &mut total_byte_size);

        NamespaceStats {
            prefix: segments
                .iter()
                .map(|segment| segment.as_ref())
                .collect::<Vec<_>>()
                .join(":"),
            key_count,
            total_byte_size,
            sub_branch_count,
            is_terminal,
        }
    }

    fn collect_subtree_stats(node: &TrieNode, key_count: &mut usize, total_byte_size: &mut usize) {
        if let Some(ref entry) = node.entry {
            *key_count = (*key_count).saturating_add(1);
            *total_byte_size = (*total_byte_size).saturating_add(entry.byte_size());
        }
        for child in node.children.values() {
            Self::collect_subtree_stats(child, key_count, total_byte_size);
        }
    }

    /// Detects prefix collisions across the entire trie.
    ///
    /// A prefix collision occurs when a node is a terminal storage entry AND
    /// has child nodes (meaning it is a prefix of deeper keys).
    pub fn detect_collisions(&self) -> Vec<PrefixCollision> {
        let mut collisions = Vec::new();
        let mut path = Vec::new();
        Self::find_collisions(&self.root, &mut path, &mut collisions);
        collisions
    }

    fn find_collisions(
        node: &TrieNode,
        path: &mut Vec<String>,
        collisions: &mut Vec<PrefixCollision>,
    ) {
        if !node.segment.is_empty() {
            path.push(node.segment.clone());
        }

        if let Some(ref entry) = node.entry {
            if !node.children.is_empty() {
                let mut deeper_keys = Vec::new();
                for child in node.children.values() {
                    Self::collect_deeper_keys(child, &mut deeper_keys);
                }
                collisions.push(PrefixCollision {
                    prefix: path.join(":"),
                    terminal_key: entry.key_identifier().to_string(),
                    deeper_keys,
                });
            }
        }

        for child in node.children.values() {
            Self::find_collisions(child, path, collisions);
        }

        if !node.segment.is_empty() {
            path.pop();
        }
    }

    fn collect_deeper_keys(node: &TrieNode, deeper_keys: &mut Vec<String>) {
        if let Some(ref entry) = node.entry {
            deeper_keys.push(entry.key_identifier().to_string());
        }
        for child in node.children.values() {
            Self::collect_deeper_keys(child, deeper_keys);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Durability, EntryType};

    fn make_entry(key: &str, bytes: usize) -> StorageEntry {
        StorageEntry::new(
            key,
            EntryType::PersistentData,
            Durability::Persistent,
            bytes,
            6_000_000,
            5_900_000,
        )
    }

    #[test]
    fn test_trie_insert_and_lookup() {
        let mut trie = KeyNamespaceTrie::new();
        trie.insert(make_entry("user:profile:100", 250));
        trie.insert(make_entry("user:profile:200", 300));
        trie.insert(make_entry("admin:config", 100));

        assert!(trie.get("user:profile:100").is_some());
        assert_eq!(trie.get("user:profile:100").unwrap().byte_size(), 250);
        assert!(trie.get("user:profile:999").is_none());
    }

    #[test]
    fn test_aggregate_namespace_size() {
        let mut trie = KeyNamespaceTrie::new();
        trie.insert(make_entry("user:profile:100", 250));
        trie.insert(make_entry("user:profile:200", 300));
        trie.insert(make_entry("user:vault:100", 500));
        trie.insert(make_entry("admin:config", 100));

        let user_profile_stats = trie.aggregate_namespace_size("user:profile");
        assert_eq!(user_profile_stats.key_count, 2);
        assert_eq!(user_profile_stats.total_byte_size, 550);
        assert_eq!(user_profile_stats.sub_branch_count, 2);

        let user_stats = trie.aggregate_namespace_size("user");
        assert_eq!(user_stats.key_count, 3);
        assert_eq!(user_stats.total_byte_size, 1050);

        let non_existent = trie.aggregate_namespace_size("finance");
        assert_eq!(non_existent.key_count, 0);
        assert_eq!(non_existent.total_byte_size, 0);
    }

    #[test]
    fn test_detect_collisions() {
        let mut trie = KeyNamespaceTrie::new();
        trie.insert(make_entry("user:profile", 150));
        trie.insert(make_entry("user:profile:settings", 200));

        let collisions = trie.detect_collisions();
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].prefix, "user:profile");
        assert_eq!(collisions[0].terminal_key, "user:profile");
        assert_eq!(collisions[0].deeper_keys, vec!["user:profile:settings"]);
    }
}
