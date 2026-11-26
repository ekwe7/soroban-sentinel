use stellar_strkey::{ed25519::PublicKey as Ed25519PublicKey, Contract};
use stellar_xdr::curr::{AccountId, Hash, PublicKey, ScAddress, Uint256};

use crate::error::{SentinelError, SentinelResult};

pub fn parse_sc_address(input: &str) -> SentinelResult<ScAddress> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SentinelError::InvalidContractAddress(
            "Address string cannot be empty".to_string(),
        ));
    }

    match trimmed.as_bytes()[0] {
        b'G' => {
            let public_key = Ed25519PublicKey::from_string(trimmed).map_err(|error| {
                SentinelError::InvalidContractAddress(format!(
                    "Invalid account strkey '{}': {}",
                    trimmed, error
                ))
            })?;
            let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(public_key.0)));
            Ok(ScAddress::Account(account_id))
        }
        b'C' => {
            let contract_id = Contract::from_string(trimmed).map_err(|error| {
                SentinelError::InvalidContractAddress(format!(
                    "Invalid contract strkey '{}': {}",
                    trimmed, error
                ))
            })?;
            Ok(ScAddress::Contract(Hash(contract_id.0)))
        }
        prefix => Err(SentinelError::InvalidContractAddress(format!(
            "Unsupported strkey prefix '{}' in '{}': expected 'G' for account or 'C' for contract",
            prefix as char, trimmed
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_and_contract_strkeys() {
        let account_bytes = [7u8; 32];
        let account = parse_sc_address(&Ed25519PublicKey(account_bytes).to_string()).unwrap();
        assert_eq!(
            account,
            ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
                account_bytes
            ))))
        );

        let contract_bytes = [8u8; 32];
        let contract = parse_sc_address(&Contract(contract_bytes).to_string()).unwrap();
        assert_eq!(contract, ScAddress::Contract(Hash(contract_bytes)));
    }

    #[test]
    fn rejects_unsupported_prefix() {
        assert!(matches!(
            parse_sc_address("SAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            Err(SentinelError::InvalidContractAddress(_))
        ));
    }
}
