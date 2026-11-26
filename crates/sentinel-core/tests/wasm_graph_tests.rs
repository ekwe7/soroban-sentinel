use sentinel_core::domain::{
    WasmDependencyGraph, WasmGraphError, WasmGraphNode, WasmRentLiability,
};

fn contract(seed: u8) -> String {
    stellar_strkey::Contract([seed; 32]).to_string()
}

#[test]
fn one_instance_one_hash_tracks_rent_and_bidirectional_edges() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [1u8; 32];
    let instance = contract(1);

    graph.register_code(hash, 1_000);
    graph.register_instance(&instance, hash, 250).unwrap();

    assert_eq!(graph.code_count(), 1);
    assert_eq!(graph.instance_count(), 1);
    assert!(!graph.is_empty());
    assert_eq!(graph.wasm_hash_for_instance(&instance), Some(hash));
    assert_eq!(
        graph.referencing_contracts(&hash).collect::<Vec<_>>(),
        vec![instance.as_str()]
    );
    assert!(graph.find_orphaned_wasm_hashes().is_empty());
    assert!(graph
        .neighbors(&WasmGraphNode::Code(hash))
        .any(|node| *node == WasmGraphNode::Instance(instance.clone())));
    assert!(graph
        .neighbors(&WasmGraphNode::Instance(instance))
        .any(|node| *node == WasmGraphNode::Code(hash)));

    let liability = graph.rent_liability_for(&hash).expect("known hash");
    assert_eq!(liability.wasm_hash, hash);
    assert_eq!(liability.referencing_instances, 1);
    assert_eq!(liability.code_monthly_rent_stroops, 1_000);
    assert_eq!(liability.instances_monthly_rent_stroops, 250);
    assert_eq!(liability.cumulative_monthly_rent_stroops, 1_250);
    assert_eq!(graph.total_rent_liability_stroops(), 1_250);
}

#[test]
fn many_instances_share_one_hash_and_rent_is_aggregated() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [7u8; 32];
    let instances: Vec<String> = (1..=3).map(contract).collect();
    graph.register_code(hash, 500);
    for (index, instance) in instances.iter().enumerate() {
        graph
            .register_instance(instance, hash, 100 * (index as u128 + 1))
            .unwrap();
    }

    assert_eq!(graph.code_count(), 1);
    assert_eq!(graph.instance_count(), 3);
    assert_eq!(graph.referencing_contracts(&hash).count(), 3);
    assert_eq!(graph.aggregate_instance_rent_for_hash(&hash), 600);

    let liability = graph.rent_liability_for(&hash).expect("known hash");
    assert_eq!(liability.referencing_instances, 3);
    assert_eq!(liability.instances_monthly_rent_stroops, 600);
    assert_eq!(liability.cumulative_monthly_rent_stroops, 1_100);

    let mut sorted = instances.iter().map(String::as_str).collect::<Vec<_>>();
    sorted.sort_unstable();
    assert_eq!(
        graph.referencing_contracts(&hash).collect::<Vec<_>>(),
        sorted
    );
    assert_eq!(graph.rent_liabilities(), vec![liability]);
}

#[test]
fn zero_instances_one_hash_is_orphaned_and_still_bills_code_rent() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [9u8; 32];
    graph.register_code(hash, 700);

    assert_eq!(graph.find_orphaned_wasm_hashes(), vec![hash]);
    assert_eq!(graph.referencing_contracts(&hash).count(), 0);
    assert_eq!(graph.aggregate_instance_rent_for_hash(&hash), 0);
    assert_eq!(graph.code_rent_for_hash(&hash), 700);

    let liability = graph.rent_liability_for(&hash).expect("known hash");
    assert_eq!(liability.referencing_instances, 0);
    assert_eq!(liability.instances_monthly_rent_stroops, 0);
    assert_eq!(liability.cumulative_monthly_rent_stroops, 700);
    assert_eq!(graph.total_rent_liability_stroops(), 700);

    let empty = WasmDependencyGraph::new();
    assert!(empty.is_empty());
    assert!(empty.find_orphaned_wasm_hashes().is_empty());
    assert!(empty.rent_liabilities().is_empty());
    assert_eq!(empty.total_rent_liability_stroops(), 0);
    assert_eq!(empty.rent_liability_for(&hash), None);
}

#[test]
fn upgrade_rebinding_orphans_the_previous_bytecode() {
    let mut graph = WasmDependencyGraph::new();
    let old_hash = [2u8; 32];
    let new_hash = [3u8; 32];
    let instance = contract(4);
    graph.register_code(old_hash, 400);
    graph.register_code(new_hash, 900);
    graph.register_instance(&instance, old_hash, 50).unwrap();

    graph.register_instance(&instance, new_hash, 60).unwrap();

    assert_eq!(graph.wasm_hash_for_instance(&instance), Some(new_hash));
    assert_eq!(graph.referencing_contracts(&old_hash).count(), 0);
    assert_eq!(graph.find_orphaned_wasm_hashes(), vec![old_hash]);
    assert_eq!(
        graph.referencing_contracts(&new_hash).collect::<Vec<_>>(),
        vec![instance.as_str()]
    );
    assert_eq!(graph.code_count(), 2);
    assert_eq!(graph.instance_count(), 1);
    assert_eq!(
        graph
            .rent_liability_for(&old_hash)
            .unwrap()
            .cumulative_monthly_rent_stroops,
        400
    );
    assert_eq!(
        graph
            .rent_liability_for(&new_hash)
            .unwrap()
            .cumulative_monthly_rent_stroops,
        960
    );
    assert_eq!(graph.total_rent_liability_stroops(), 1_360);
}

#[test]
fn re_registering_an_instance_does_not_duplicate_edges_or_rent() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [5u8; 32];
    let instance = contract(5);
    graph.register_instance(&instance, hash, 100).unwrap();
    graph.register_instance(&instance, hash, 150).unwrap();

    assert_eq!(graph.instance_count(), 1);
    assert_eq!(graph.referencing_contracts(&hash).count(), 1);
    assert_eq!(graph.aggregate_instance_rent_for_hash(&hash), 150);
    assert_eq!(
        graph.neighbors(&WasmGraphNode::Instance(instance)).count(),
        1
    );
}

#[test]
fn unlink_keeps_bytecode_entry_and_marks_it_orphaned() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [6u8; 32];
    let instance = contract(6);
    graph.register_code(hash, 200);
    graph.register_instance(&instance, hash, 30).unwrap();

    assert!(graph.unlink_instance(&instance));
    assert!(!graph.unlink_instance(&instance));
    assert_eq!(graph.instance_count(), 0);
    assert_eq!(graph.wasm_hash_for_instance(&instance), None);
    assert_eq!(graph.find_orphaned_wasm_hashes(), vec![hash]);
    assert_eq!(graph.total_rent_liability_stroops(), 200);
}

#[test]
fn orphaned_hashes_liabilities_and_neighbors_are_deterministically_ordered() {
    let mut graph = WasmDependencyGraph::new();
    for byte in [3u8, 1, 2] {
        graph.register_code([byte; 32], u128::from(byte));
    }
    let ordered = [[1u8; 32], [2u8; 32], [3u8; 32]];

    assert_eq!(graph.wasm_hashes().collect::<Vec<_>>(), ordered);
    assert_eq!(graph.find_orphaned_wasm_hashes(), ordered);
    assert_eq!(
        graph
            .rent_liabilities()
            .iter()
            .map(|liability| liability.wasm_hash)
            .collect::<Vec<_>>(),
        ordered
    );

    let neighbors = graph
        .neighbors(&WasmGraphNode::Code([2u8; 32]))
        .cloned()
        .collect::<Vec<_>>();
    assert!(neighbors.is_empty());
}

#[test]
fn rent_totals_saturate_instead_of_overflowing() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [8u8; 32];
    graph.register_code(hash, u128::MAX);
    graph.register_instance(contract(8), hash, 1).unwrap();
    graph
        .register_instance(contract(9), hash, u128::MAX)
        .unwrap();

    let liability = graph.rent_liability_for(&hash).unwrap();
    assert_eq!(liability.instances_monthly_rent_stroops, u128::MAX);
    assert_eq!(liability.cumulative_monthly_rent_stroops, u128::MAX);
    assert_eq!(graph.total_rent_liability_stroops(), u128::MAX);
}

#[test]
fn instance_without_explicit_code_registration_is_tracked() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [4u8; 32];
    let instance = contract(7);
    graph.register_instance(&instance, hash, 10).unwrap();

    assert_eq!(graph.code_rent_for_hash(&hash), 0);
    assert!(graph.find_orphaned_wasm_hashes().is_empty());
    assert_eq!(
        graph
            .rent_liability_for(&hash)
            .unwrap()
            .cumulative_monthly_rent_stroops,
        10
    );
}

#[test]
fn contract_ids_are_canonicalized_before_indexing() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [12u8; 32];
    let instance = contract(12);

    graph
        .register_instance(format!("  {instance}  "), hash, 20)
        .unwrap();

    assert_eq!(graph.wasm_hash_for_instance(&instance), Some(hash));
    assert_eq!(graph.instance_count(), 1);
    assert_eq!(
        graph.referencing_contracts(&hash).collect::<Vec<_>>(),
        vec![instance.as_str()]
    );
    assert!(graph.unlink_instance(&instance));
}

#[test]
fn empty_contract_ids_are_rejected() {
    let mut graph = WasmDependencyGraph::new();
    for id in ["", "   "] {
        assert_eq!(
            graph.register_instance(id, [1u8; 32], 10),
            Err(WasmGraphError::EmptyContractId)
        );
    }
    assert!(graph.is_empty());
    assert!(graph.register_instance("valid", [1u8; 32], 10).is_ok());
}

#[test]
fn rent_liability_serializes_and_round_trips() {
    let mut graph = WasmDependencyGraph::new();
    let hash = [11u8; 32];
    graph.register_code(hash, 1_000);
    graph.register_instance(contract(11), hash, 250).unwrap();
    let liability = graph.rent_liability_for(&hash).unwrap();

    let json = serde_json::to_string(&liability).expect("serialize liability");
    let restored: WasmRentLiability = serde_json::from_str(&json).expect("deserialize liability");
    assert_eq!(restored, liability);
}
