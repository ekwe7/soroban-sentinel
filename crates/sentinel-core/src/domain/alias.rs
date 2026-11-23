//! Local contract alias registry.
//!
//! Operators can refer to Soroban contracts by short, memorable names such as
//! `treasury` or `pool-usdc` instead of 56-character `C...` strkeys. The
//! registry validates every target as a contract strkey, rejects duplicate
//! aliases and duplicate targets, and offers "did you mean?" suggestions for
//! unknown names using a small Levenshtein distance helper.
//!
//! This module is pure: it performs no I/O and no remote lookups.

use std::collections::BTreeMap;

/// Maximum length of an alias name.
pub const MAX_ALIAS_LEN: usize = 64;

/// Largest edit distance for which a typo suggestion is offered.
pub const MAX_SUGGESTION_DISTANCE: usize = 2;

/// Errors produced while building or querying an [`AliasRegistry`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AliasError {
    /// The alias name is empty, too long, or contains unsupported characters.
    #[error("invalid alias name `{0}`: use 1-64 letters, digits, '-' or '_', starting with a letter or digit")]
    InvalidName(String),
    /// The alias target is not a valid `C...` contract strkey.
    #[error("alias `{alias}` target `{target}` is not a valid contract strkey (C...)")]
    InvalidTarget {
        /// Alias that was being registered.
        alias: String,
        /// Offending target value.
        target: String,
    },
    /// The alias name is already registered.
    #[error("duplicate alias `{0}`")]
    DuplicateAlias(String),
    /// The target contract is already registered under another alias.
    #[error("alias `{alias}` points at a contract already registered as `{existing}`")]
    DuplicateTarget {
        /// Alias that was being registered.
        alias: String,
        /// Alias that already owns the target.
        existing: String,
    },
    /// The name is not a strkey or registered alias, but closely matches one.
    #[error("unknown contract alias `{alias}`; did you mean `{suggestion}`?")]
    UnknownWithSuggestion {
        /// Name that was looked up.
        alias: String,
        /// Closest registered alias.
        suggestion: String,
    },
    /// The name is not a strkey or registered alias and has no close match.
    #[error("unknown contract alias `{0}`")]
    Unknown(String),
}

/// Returns true when `value` is a valid `C...` Soroban contract strkey.
pub fn is_contract_strkey(value: &str) -> bool {
    stellar_strkey::Contract::from_string(value).is_ok()
}

/// Computes the Levenshtein edit distance between two strings, counting
/// insertions, deletions and substitutions of Unicode scalar values.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn validate_name(name: &str) -> Result<(), AliasError> {
    let well_formed = !name.is_empty()
        && name.len() <= MAX_ALIAS_LEN
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    // An alias must never be confusable with a real contract strkey.
    if !well_formed || is_contract_strkey(name) {
        return Err(AliasError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Maps alias names to validated, canonical `C...` contract IDs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliasRegistry {
    entries: BTreeMap<String, String>,
}

impl AliasRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a registry from `(alias, target)` pairs, failing on the first
    /// invalid name, invalid target, duplicate alias or duplicate target.
    pub fn from_pairs<I, K, V>(pairs: I) -> Result<Self, AliasError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut registry = Self::new();
        for (alias, target) in pairs {
            registry.insert(alias.as_ref(), target.as_ref())?;
        }
        Ok(registry)
    }

    /// Registers `alias` for the contract strkey `target`.
    pub fn insert(&mut self, alias: &str, target: &str) -> Result<(), AliasError> {
        validate_name(alias)?;
        let target = target.trim();
        let canonical = stellar_strkey::Contract::from_string(target)
            .map_err(|_| AliasError::InvalidTarget {
                alias: alias.to_string(),
                target: target.to_string(),
            })?
            .to_string();
        if self.entries.contains_key(alias) {
            return Err(AliasError::DuplicateAlias(alias.to_string()));
        }
        if let Some((existing, _)) = self.entries.iter().find(|(_, id)| **id == canonical) {
            return Err(AliasError::DuplicateTarget {
                alias: alias.to_string(),
                existing: existing.clone(),
            });
        }
        self.entries.insert(alias.to_string(), canonical);
        Ok(())
    }

    /// Returns the contract ID registered for `alias`, if any.
    pub fn get(&self, alias: &str) -> Option<&str> {
        self.entries.get(alias).map(String::as_str)
    }

    /// Returns true when `alias` is registered.
    pub fn contains(&self, alias: &str) -> bool {
        self.entries.contains_key(alias)
    }

    /// Number of registered aliases.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true when no aliases are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates `(alias, contract_id)` pairs in alphabetical alias order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(a, c)| (a.as_str(), c.as_str()))
    }

    /// Suggests the closest registered alias to `unknown` when its
    /// case-insensitive edit distance is at most [`MAX_SUGGESTION_DISTANCE`].
    /// Ties resolve alphabetically so the result is deterministic.
    pub fn suggest(&self, unknown: &str) -> Option<&str> {
        let needle = unknown.to_lowercase();
        self.entries
            .keys()
            .map(|name| (levenshtein(&name.to_lowercase(), &needle), name.as_str()))
            .filter(|(distance, _)| *distance <= MAX_SUGGESTION_DISTANCE)
            .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .map(|(_, name)| name)
    }

    /// Resolves `target` to a contract ID. A valid `C...` strkey passes
    /// straight through; otherwise `target` is looked up as an alias.
    pub fn resolve(&self, target: &str) -> Result<String, AliasError> {
        let target = target.trim();
        if let Ok(contract) = stellar_strkey::Contract::from_string(target) {
            return Ok(contract.to_string());
        }
        if let Some(id) = self.get(target) {
            return Ok(id.to_string());
        }
        match self.suggest(target) {
            Some(suggestion) => Err(AliasError::UnknownWithSuggestion {
                alias: target.to_string(),
                suggestion: suggestion.to_string(),
            }),
            None => Err(AliasError::Unknown(target.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> String {
        stellar_strkey::Contract([n; 32]).to_string()
    }

    fn registry() -> AliasRegistry {
        AliasRegistry::from_pairs([("treasury", id(1)), ("pool-usdc", id(2))]).unwrap()
    }

    #[test]
    fn exact_alias_resolves_to_its_contract() {
        let registry = registry();
        assert_eq!(registry.resolve("treasury").unwrap(), id(1));
        assert_eq!(registry.resolve("pool-usdc").unwrap(), id(2));
        assert_eq!(registry.get("treasury"), Some(id(1).as_str()));
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
    }

    #[test]
    fn direct_strkey_passes_through_even_with_an_empty_registry() {
        assert_eq!(AliasRegistry::new().resolve(&id(9)).unwrap(), id(9));
        assert_eq!(
            registry().resolve(&format!("  {}  ", id(9))).unwrap(),
            id(9)
        );
    }

    #[test]
    fn typo_gets_a_did_you_mean_suggestion() {
        let error = registry().resolve("treasuri").unwrap_err();
        assert_eq!(
            error,
            AliasError::UnknownWithSuggestion {
                alias: "treasuri".into(),
                suggestion: "treasury".into()
            }
        );
        assert!(error.to_string().contains("did you mean `treasury`?"));
    }

    #[test]
    fn suggestion_boundary_is_distance_two() {
        assert_eq!(registry().suggest("treasxrx"), Some("treasury"));
        assert_eq!(registry().suggest("treasxxx"), None);
        assert_eq!(
            registry().resolve("zzzzzz").unwrap_err(),
            AliasError::Unknown("zzzzzz".into())
        );
    }

    #[test]
    fn suggestions_ignore_case_and_break_ties_alphabetically() {
        assert_eq!(registry().suggest("Treasury"), Some("treasury"));
        let tied = AliasRegistry::from_pairs([("aac", id(1)), ("aab", id(2))]).unwrap();
        assert_eq!(tied.suggest("aad"), Some("aab"));
    }

    #[test]
    fn invalid_targets_are_rejected() {
        let mut registry = AliasRegistry::new();
        let account = stellar_strkey::ed25519::PublicKey([3; 32]).to_string();
        for target in ["not-a-strkey", "", account.as_str()] {
            assert!(matches!(
                registry.insert("bad", target),
                Err(AliasError::InvalidTarget { .. })
            ));
        }
        assert!(registry.is_empty());
    }

    #[test]
    fn duplicate_alias_and_duplicate_target_are_rejected() {
        let mut registry = AliasRegistry::new();
        registry.insert("a", &id(1)).unwrap();
        assert_eq!(
            registry.insert("a", &id(2)),
            Err(AliasError::DuplicateAlias("a".into()))
        );
        assert_eq!(
            registry.insert("b", &id(1)),
            Err(AliasError::DuplicateTarget {
                alias: "b".into(),
                existing: "a".into()
            })
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn invalid_alias_names_are_rejected() {
        let long = "a".repeat(MAX_ALIAS_LEN + 1);
        let strkey_name = id(4);
        for name in [
            "",
            " x",
            "-x",
            "has space",
            "a/b",
            long.as_str(),
            strkey_name.as_str(),
        ] {
            assert!(
                matches!(
                    AliasRegistry::new().insert(name, &id(1)),
                    Err(AliasError::InvalidName(_))
                ),
                "name {name:?} should be rejected"
            );
        }
        let longest = "a".repeat(MAX_ALIAS_LEN);
        assert!(AliasRegistry::new().insert(&longest, &id(1)).is_ok());
    }

    #[test]
    fn levenshtein_matches_known_distances() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("same", "same"), 0);
        assert_eq!(levenshtein("é", "e"), 1);
    }
}
