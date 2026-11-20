
fn resolve_contracts(args: &BundleArgs) -> Result<Vec<ContractInput>, BundleCommandError> {
    let mut contracts = Vec::<ContractInput>::new();

    // Load contracts from the optional JSON manifest.
    if let Some(path) = &args.manifest {
        let file = fs::File::open(path).map_err(|error| {
            BundleCommandError::Invalid(format!(
                "failed to read manifest '{}': {error}",
                path.display()
            ))
        })?;

        let mut contents = Vec::new();

        file.take(MANIFEST_MAX_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| {
                BundleCommandError::Invalid(format!(
                    "failed to read manifest '{}': {error}",
                    path.display()
                ))
            })?;

        if contents.len() as u64 > MANIFEST_MAX_BYTES {
            return Err(BundleCommandError::Invalid(format!(
                "manifest '{}' exceeds the {} byte limit",
                path.display(),
                MANIFEST_MAX_BYTES
            )));
        }

        let manifest: BundleManifest = serde_json::from_slice(&contents).map_err(|error| {
            BundleCommandError::Invalid(format!(
                "invalid JSON bundle manifest '{}': {error}",
                path.display()
            ))
        })?;

        for entry in manifest.contracts {
            merge_contract(
                &mut contracts,
                &entry.contract_id,
                &entry.keys,
                &entry.key_xdr,
            )?;
        }
    }

    // Merge contracts provided directly through CLI arguments.
    for contract in &args.contracts {
        merge_contract(&mut contracts, contract, &[], &[])?;
    }

    if contracts.is_empty() {
        return Err(BundleCommandError::Invalid(
            "provide at least one --contract or a manifest with contracts"
                .to_string(),
        ));
    }

    // Apply command-line symbol keys to every selected contract.
    for contract in &mut contracts {
        for key in &args.keys {
            if !contract.keys.contains(key) {
                contract.keys.push(key.clone());
            }
        }

        // Apply XDR keys independently of symbol keys.
        for key_xdr in &args.key_xdr {
            if !contract.key_xdr.contains(key_xdr) {
                contract.key_xdr.push(key_xdr.clone());
            }
        }
    }

    Ok(contracts)
}
