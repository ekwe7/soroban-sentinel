//! Detect logical key aliases across Soroban storage durability namespaces.

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{Limits, ReadXdr, ScVal, WriteXdr};

use crate::{
    adapters::parse_key_expr,
    domain::{Durability, EntryType, StorageEntry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCollisionKind {
    /// The same canonical ScVal key is present in persistent and temporary storage.
    CrossDurability,
    /// Raw bytes have the same content as a normalized string or symbol key.
    RawBytesSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCollisionWarning {
    pub first_key: String,
    pub second_key: String,
    pub first_durability: Durability,
    pub second_durability: Durability,
    pub kind: KeyCollisionKind,
}

/// Finds exact cross-durability keys and byte/string-symbol aliases.
///
/// Identifiers produced by scans (`persistent:NAME`, typed expressions, and
/// `*_xdr:<base64>` forms) are decoded to ScVal and compared by canonical XDR.
/// Entries whose key identifiers do not preserve enough key data are skipped.
pub fn detect_shadowed_keys(entries: &[StorageEntry]) -> Vec<KeyCollisionWarning> {
    let parsed: Vec<_> = entries
        .iter()
        .filter(|entry| entry.entry_type() == EntryType::PersistentData)
        .filter_map(|entry| parse_identifier(entry.key_identifier()).map(|key| (entry, key)))
        .collect();
    let mut warnings = Vec::new();

    for left in 0..parsed.len() {
        for right in left + 1..parsed.len() {
            let (first, first_key) = &parsed[left];
            let (second, second_key) = &parsed[right];
            let same_key = canonical_xdr(first_key)
                .zip(canonical_xdr(second_key))
                .is_some_and(|(first, second)| first == second);
            let cross_durability = first.durability() != second.durability()
                && matches!(
                    (first.durability(), second.durability()),
                    (Durability::Persistent, Durability::Temporary)
                        | (Durability::Temporary, Durability::Persistent)
                );
            if same_key && cross_durability {
                warnings.push(warning(first, second, KeyCollisionKind::CrossDurability));
                continue;
            }
            if is_bytes_symbol_alias(first_key, second_key) {
                warnings.push(warning(first, second, KeyCollisionKind::RawBytesSymbol));
            }
        }
    }
    warnings.sort_by(|left, right| {
        left.first_key
            .cmp(&right.first_key)
            .then_with(|| left.second_key.cmp(&right.second_key))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    warnings
}

fn warning(
    first: &StorageEntry,
    second: &StorageEntry,
    kind: KeyCollisionKind,
) -> KeyCollisionWarning {
    KeyCollisionWarning {
        first_key: first.key_identifier().to_owned(),
        second_key: second.key_identifier().to_owned(),
        first_durability: first.durability(),
        second_durability: second.durability(),
        kind,
    }
}

fn parse_identifier(identifier: &str) -> Option<ScVal> {
    for prefix in ["persistent_xdr:", "temporary_xdr:"] {
        if let Some(encoded) = identifier.strip_prefix(prefix) {
            return ScVal::from_xdr_base64(encoded, Limits::none()).ok();
        }
    }
    let key = identifier
        .strip_prefix("persistent:")
        .or_else(|| identifier.strip_prefix("temporary:"))
        .unwrap_or(identifier);
    let expression = if key.contains(':') || key.starts_with("vec[") {
        key.to_owned()
    } else {
        format!("sym:{key}")
    };
    parse_key_expr(&expression).ok()
}

fn canonical_xdr(value: &ScVal) -> Option<Vec<u8>> {
    value.to_xdr(Limits::none()).ok()
}

fn normalized_text(value: &ScVal) -> Option<Vec<u8>> {
    match value {
        ScVal::Symbol(value) => Some(value.0.to_string().into_bytes()),
        ScVal::String(value) => Some(value.0.to_string().into_bytes()),
        ScVal::Bytes(value) => Some(value.0.to_vec()),
        _ => None,
    }
}

fn is_bytes_symbol_alias(left: &ScVal, right: &ScVal) -> bool {
    matches!(
        (left, right),
        (ScVal::Bytes(_), ScVal::Symbol(_) | ScVal::String(_))
            | (ScVal::Symbol(_) | ScVal::String(_), ScVal::Bytes(_))
    ) && normalized_text(left).is_some()
        && normalized_text(left) == normalized_text(right)
}
