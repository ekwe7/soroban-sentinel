//! Operator-driven TTL bump command.

use ed25519_dalek::Signer;
use stellar_strkey::ed25519::{PrivateKey, PublicKey};
use stellar_xdr::curr::{
    DecoratedSignature, Limits, ReadXdr, Signature, SignatureHint, TransactionEnvelope, VecM,
    WriteXdr,
};

use crate::args::{BumpArgs, BundleArgs};
use crate::commands::bundle::run_bundle_inner;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE};

pub async fn run_bump(args: BumpArgs) -> i32 {
    if args.extend_to == 0 {
        eprintln!("Error: --extend-to must be greater than 0");
        return EXIT_INVALID_ARGS;
    }
    if args.submit && args.max_fee.is_none() {
        eprintln!("Error: --max-fee is required with --submit");
        return EXIT_INVALID_ARGS;
    }

    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    let passphrase = match args
        .network_passphrase
        .clone()
        .or_else(|| match args.network.as_str() {
            "testnet" => Some("Test SDF Network ; September 2015".to_string()),
            "mainnet" => Some("Public Global Stellar Network ; September 2015".to_string()),
            "futurenet" => Some("Test SDF Future Network ; October 2022".to_string()),
            "local" => Some("Standalone Network ; February 2017".to_string()),
            _ => None,
        }) {
        Some(value) => value,
        None => {
            eprintln!("Error: --network-passphrase is required when --rpc-url is used with an unknown network");
            return EXIT_INVALID_ARGS;
        }
    };

    let network_report = reqwest::Client::new()
        .post(&rpc_url)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"getNetwork","params":{}}))
        .send()
        .await;
    if let Ok(response) = network_report {
        if let Ok(value) = response.json::<serde_json::Value>().await {
            if let Some(reported) = value
                .pointer("/result/networkPassphrase")
                .and_then(|v| v.as_str())
            {
                if reported != passphrase {
                    eprintln!(
                        "Error: network passphrase mismatch: RPC reports '{}', configured '{}'",
                        reported, passphrase
                    );
                    return EXIT_INVALID_ARGS;
                }
            }
        }
    }

    let bundle_args = BundleArgs {
        contracts: vec![args.contract_id.clone()],
        keys: args.key.clone(),
        key_xdr: args.key_xdr.clone(),
        manifest: None,
        source_account: args.source_account.clone(),
        extend_by: args.extend_to,
        base_fee: 100,
        network: args.network.clone(),
        rpc_url: Some(rpc_url.clone()),
    };

    let output = match run_bundle_inner(bundle_args).await {
        Ok(output) => output,
        Err(error) => {
            eprintln!("Error: {error:?}");
            return EXIT_RPC_FAILURE;
        }
    };

    if output.total_fee_stroops > args.max_fee.unwrap_or(u64::MAX) && args.submit {
        eprintln!(
            "Error: simulated fee {} exceeds --max-fee {}",
            output.total_fee_stroops,
            args.max_fee.unwrap_or_default()
        );
        return EXIT_INVALID_ARGS;
    }

    let mut signed_envelopes = Vec::new();
    if args.submit || std::env::var_os("SENTINEL_SECRET_KEY").is_some() {
        let secret = match std::env::var("SENTINEL_SECRET_KEY") {
            Ok(secret) => secret,
            Err(_) if args.submit => {
                eprintln!("Error: SENTINEL_SECRET_KEY is required with --submit");
                return EXIT_INVALID_ARGS;
            }
            Err(_) => String::new(),
        };

        if args.submit {
            let private = match PrivateKey::from_string(&secret) {
                Ok(key) => key,
                Err(_) => {
                    eprintln!("Error: SENTINEL_SECRET_KEY is not a valid Stellar secret key");
                    return EXIT_INVALID_ARGS;
                }
            };
            let mut seed = private.0;
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
            seed.fill(0);

            for transaction in &output.transactions {
                let mut envelope = TransactionEnvelope::from_xdr_base64(
                    &transaction.transaction_xdr_base64,
                    Limits::none(),
                )
                .expect("bundle output must contain valid envelope XDR");
                let message = format!("{}{}", passphrase, transaction.transaction_xdr_base64);
                let signature = signing_key.sign(message.as_bytes());
                let public = PublicKey::from_payload(&signing_key.verifying_key().to_bytes())
                    .expect("valid public key");
                let hint = public.0[28..]
                    .try_into()
                    .expect("32-byte public key hint slice");

                if let TransactionEnvelope::Tx(v1) = &mut envelope {
                    let mut signatures = v1.signatures.to_vec();
                    signatures.push(DecoratedSignature {
                        hint: SignatureHint(hint),
                        signature: Signature(
                            signature
                                .to_bytes()
                                .to_vec()
                                .try_into()
                                .expect("64-byte signature"),
                        ),
                    });
                    v1.signatures = VecM::try_from(signatures).expect("valid signature list");
                }
                signed_envelopes.push(
                    envelope
                        .to_xdr_base64(Limits::none())
                        .expect("valid signed envelope"),
                );
            }
        }
    }

    if args.submit {
        for envelope in &signed_envelopes {
            let response = reqwest::Client::new()
                .post(&rpc_url)
                .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":{"transaction":envelope}}))
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let value = response
                        .json::<serde_json::Value>()
                        .await
                        .unwrap_or_default();
                    if value.get("error").is_some() {
                        eprintln!("Error: submitTransaction failed: {}", value["error"]);
                        return EXIT_RPC_FAILURE;
                    }
                }
                Ok(response) => {
                    eprintln!("Error: submitTransaction HTTP {}", response.status());
                    return EXIT_RPC_FAILURE;
                }
                Err(error) => {
                    eprintln!("Error: submitTransaction request failed: {error}");
                    return EXIT_RPC_FAILURE;
                }
            }
        }
    }

    let envelopes = if args.submit {
        signed_envelopes.clone()
    } else {
        output
            .transactions
            .iter()
            .map(|transaction| transaction.transaction_xdr_base64.clone())
            .collect()
    };
    println!(
        "{{\"mode\":\"{}\",\"simulated_fee_stroops\":{},\"submitted\":{},\"envelopes\":{} }}",
        if args.submit { "submit" } else { "dry-run" },
        output.total_fee_stroops,
        args.submit,
        serde_json::to_string(&envelopes).unwrap_or_else(|_| "[]".to_string())
    );

    0
}
