use sentinel_core::domain::{
    Durability, EntryType, KeyNamespaceTrie, NamespaceStats, StorageEntry,
};

fn sample_entry(key: &str, byte_size: usize) -> StorageEntry {
    StorageEntry::new(
        key,
        EntryType::PersistentData,
        Durability::Persistent,
        byte_size,
        6_000_000,
        5_900_000,
    )
}

#[test]
fn test_hierarchical_key_tree_indexing() {
    let mut trie = KeyNamespaceTrie::new();

    trie.insert(sample_entry("user:profile:alice", 120));
    trie.insert(sample_entry("user:profile:bob", 140));
    trie.insert(sample_entry("user:settings:theme", 50));
    trie.insert(sample_entry("admin:roles:super", 80));
    trie.insert(sample_entry("metadata", 30));

    // Lookup exact keys
    assert_eq!(
        trie.get("user:profile:alice").map(|e| e.byte_size()),
        Some(120)
    );
    assert_eq!(
        trie.get("user:profile:bob").map(|e| e.byte_size()),
        Some(140)
    );
    assert_eq!(
        trie.get("user:settings:theme").map(|e| e.byte_size()),
        Some(50)
    );
    assert_eq!(
        trie.get("admin:roles:super").map(|e| e.byte_size()),
        Some(80)
    );
    assert_eq!(trie.get("metadata").map(|e| e.byte_size()), Some(30));

    // Non-existent keys
    assert!(trie.get("user:profile:charlie").is_none());
    assert!(trie.get("admin:roles").is_none());
}

#[test]
fn test_prefix_branch_queries_and_aggregation() {
    let mut trie = KeyNamespaceTrie::new();

    trie.insert(sample_entry("pool:liquidity:usdc_xlm", 500));
    trie.insert(sample_entry("pool:liquidity:btc_xlm", 600));
    trie.insert(sample_entry("pool:fee_tier:tier1", 100));
    trie.insert(sample_entry("pool:fee_tier:tier2", 150));
    trie.insert(sample_entry("governance:proposal:1", 200));

    // Query "pool:liquidity" prefix
    let liquidity_stats = trie.aggregate_namespace_size("pool:liquidity");
    assert_eq!(liquidity_stats.prefix, "pool:liquidity");
    assert_eq!(liquidity_stats.key_count, 2);
    assert_eq!(liquidity_stats.total_byte_size, 1100);
    assert_eq!(liquidity_stats.sub_branch_count, 2);
    assert!(!liquidity_stats.is_terminal);

    // Query "pool:fee_tier" prefix
    let fee_stats = trie.aggregate_namespace_size("pool:fee_tier");
    assert_eq!(fee_stats.key_count, 2);
    assert_eq!(fee_stats.total_byte_size, 250);

    // Query root "pool" prefix branch
    let pool_stats = trie.aggregate_namespace_size("pool");
    assert_eq!(pool_stats.key_count, 4);
    assert_eq!(pool_stats.total_byte_size, 1350);
    assert_eq!(pool_stats.sub_branch_count, 2); // liquidity and fee_tier

    // Query root prefix "" (all entries in trie)
    let total_stats = trie.aggregate_namespace_size("");
    assert_eq!(total_stats.key_count, 5);
    assert_eq!(total_stats.total_byte_size, 1550);

    // Query non-existent branch
    let empty_stats = trie.aggregate_namespace_size("oracle");
    assert_eq!(
        empty_stats,
        NamespaceStats {
            prefix: "oracle".to_string(),
            key_count: 0,
            total_byte_size: 0,
            sub_branch_count: 0,
            is_terminal: false,
        }
    );
}

#[test]
fn test_prefix_collision_detection() {
    let mut trie = KeyNamespaceTrie::new();

    // 1. Insert harmless hierarchical keys
    trie.insert(sample_entry("user:account:101", 200));
    trie.insert(sample_entry("user:account:102", 200));
    assert!(trie.detect_collisions().is_empty());

    // 2. Introduce a collision: "user:account" is now a terminal key itself,
    // shadowing the "user:account:101" and "user:account:102" subtree!
    trie.insert(sample_entry("user:account", 150));

    let collisions = trie.detect_collisions();
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].prefix, "user:account");
    assert_eq!(collisions[0].terminal_key, "user:account");
    assert_eq!(collisions[0].deeper_keys.len(), 2);
    assert!(collisions[0]
        .deeper_keys
        .contains(&"user:account:101".to_string()));
    assert!(collisions[0]
        .deeper_keys
        .contains(&"user:account:102".to_string()));

    // 3. Introduce another nested collision: "user" as a root key
    trie.insert(sample_entry("user", 80));
    let multi_collisions = trie.detect_collisions();
    assert_eq!(multi_collisions.len(), 2);
}

#[test]
fn test_slash_and_colon_delimiters() {
    let mut trie = KeyNamespaceTrie::new();
    trie.insert(sample_entry("vault/shares/user1", 300));
    trie.insert(sample_entry("vault/shares/user2", 300));

    let stats = trie.aggregate_namespace_size("vault:shares");
    assert_eq!(stats.key_count, 2);
    assert_eq!(stats.total_byte_size, 600);
}

#[test]
fn test_explicit_xdr_component_indexing() {
    let mut trie = KeyNamespaceTrie::new();
    trie.insert_with_segments(
        &["ScVal::Symbol".to_string(), "BALANCE".to_string()],
        sample_entry("xdr-key-1", 42),
    );

    assert_eq!(
        trie.get_with_segments(&["ScVal::Symbol", "BALANCE"])
            .map(|entry| entry.byte_size()),
        Some(42)
    );
    let stats = trie.aggregate_namespace_size_segments(&["ScVal::Symbol"]);
    assert_eq!(stats.key_count, 1);
    assert_eq!(stats.total_byte_size, 42);
}
