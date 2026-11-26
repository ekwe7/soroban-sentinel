//! Pure domain models for Soroban contract event streams and State Archival signals.

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::ScVal;

/// Event source classification reported by Soroban RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SorobanEventType {
    /// Protocol-emitted system event (including State Archival lifecycle events).
    System,
    /// Event emitted by a contract invocation.
    Contract,
    /// Diagnostic event emitted during contract execution.
    Diagnostic,
    /// Unrecognized event type returned by a newer RPC implementation.
    Unknown,
}

impl SorobanEventType {
    /// Parses the lowercase RPC `type` field.
    pub fn from_rpc_str(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "system" => Self::System,
            "contract" => Self::Contract,
            "diagnostic" => Self::Diagnostic,
            _ => Self::Unknown,
        }
    }
}

/// State Archival lifecycle signal inferred from contract or system event topics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateArchivalEventKind {
    /// A TTL bump/extend operation was observed.
    TtlExtension,
    /// An entry (or contract) was archived.
    Archived,
    /// An archived entry (or contract) was restored.
    Restored,
    /// An unrelated event.
    Other,
}

/// Decoded event fields ready for pure domain classification.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedContractEvent {
    /// Unique event identifier from RPC.
    pub id: String,
    /// Opaque pagination cursor for the event.
    pub paging_token: Option<String>,
    /// Ledger sequence that emitted the event.
    pub ledger: u32,
    /// Ledger close time as Unix seconds.
    pub ledger_close_time: Option<i64>,
    /// RPC event source classification.
    pub event_type: SorobanEventType,
    /// Contract that emitted or is associated with the event.
    pub contract_id: Option<String>,
    /// Decoded topic values.
    pub topics: Vec<ScVal>,
    /// Decoded event payload value.
    pub value: Option<ScVal>,
    /// Transaction hash that produced the event.
    pub tx_hash: Option<String>,
    /// Whether the event came from a successful contract call.
    pub in_successful_contract_call: bool,
}

/// Parsed contract event with State Archival classification metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractEvent {
    /// Unique event identifier from RPC.
    pub id: String,
    /// Opaque pagination cursor for the event.
    pub paging_token: Option<String>,
    /// Ledger sequence that emitted the event.
    pub ledger: u32,
    /// Ledger close time as Unix seconds.
    pub ledger_close_time: Option<i64>,
    /// Event source classification.
    pub event_type: SorobanEventType,
    /// Contract that emitted or is associated with the event.
    pub contract_id: Option<String>,
    /// Human-readable symbol/string values found in the event topics.
    pub topic_symbols: Vec<String>,
    /// Human-readable symbol/string values found in the event payload.
    pub value_symbols: Vec<String>,
    /// Inferred State Archival lifecycle signal.
    pub kind: StateArchivalEventKind,
    /// Transaction hash that produced the event.
    pub tx_hash: Option<String>,
    /// Whether the event came from a successful contract call.
    pub in_successful_contract_call: bool,
}

impl ContractEvent {
    /// Classifies decoded topics and payload without performing any I/O.
    pub fn from_decoded(event: DecodedContractEvent) -> Self {
        let topic_symbols = collect_text(&event.topics);
        let value_symbols = event
            .value
            .as_ref()
            .map(|value| collect_text(std::slice::from_ref(value)))
            .unwrap_or_default();
        let kind =
            classify_state_archival_event(&event.topics, event.value.as_ref(), event.event_type);

        Self {
            id: event.id,
            paging_token: event.paging_token,
            ledger: event.ledger,
            ledger_close_time: event.ledger_close_time,
            event_type: event.event_type,
            contract_id: event.contract_id,
            topic_symbols,
            value_symbols,
            kind,
            tx_hash: event.tx_hash,
            in_successful_contract_call: event.in_successful_contract_call,
        }
    }

    /// Returns true when the event represents a TTL extension, archival, or restore.
    pub fn is_state_archival(&self) -> bool {
        self.kind != StateArchivalEventKind::Other
    }
}

/// Infers the State Archival lifecycle signal from decoded event topics and payload.
///
/// Classification is deliberately tolerant of protocol-specific event names:
/// topics are flattened into lowercase text and matched for restore, archive,
/// extension, bump, and TTL keywords. Restore wins over archive so names such
/// as `unarchive` are not misclassified.
pub fn classify_state_archival_event(
    topics: &[ScVal],
    value: Option<&ScVal>,
    event_type: SorobanEventType,
) -> StateArchivalEventKind {
    let mut text = collect_text(topics);
    if let Some(value) = value {
        text.extend(collect_text(std::slice::from_ref(value)));
    }
    if text.is_empty() {
        return StateArchivalEventKind::Other;
    }

    let mut normalized = format!("{:?}", event_type).to_ascii_lowercase();
    for part in text {
        normalized.push(' ');
        normalized.push_str(&part.to_ascii_lowercase());
    }

    if normalized.contains("restore") || normalized.contains("unarchiv") {
        StateArchivalEventKind::Restored
    } else if normalized.contains("archiv") {
        StateArchivalEventKind::Archived
    } else if normalized.contains("extend")
        || normalized.contains("bump")
        || normalized.contains("ttl")
    {
        StateArchivalEventKind::TtlExtension
    } else {
        StateArchivalEventKind::Other
    }
}

fn collect_text(values: &[ScVal]) -> Vec<String> {
    let mut text = Vec::new();
    for value in values {
        collect_scval_text(value, &mut text);
    }
    text
}

fn collect_scval_text(value: &ScVal, text: &mut Vec<String>) {
    match value {
        ScVal::Symbol(symbol) => text.push(symbol.0.to_string()),
        ScVal::String(string) => text.push(string.0.to_string()),
        ScVal::Vec(Some(vector)) => {
            for item in vector.0.iter() {
                collect_scval_text(item, text);
            }
        }
        ScVal::Map(Some(map)) => {
            for entry in map.0.iter() {
                collect_scval_text(&entry.key, text);
                collect_scval_text(&entry.val, text);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{ScMap, ScMapEntry, ScString, ScSymbol, ScVec, VecM};

    fn symbol(value: &str) -> ScVal {
        ScVal::Symbol(ScSymbol(value.try_into().unwrap()))
    }

    fn string(value: &str) -> ScVal {
        ScVal::String(ScString(value.parse().unwrap()))
    }

    fn vec(values: Vec<ScVal>) -> ScVal {
        ScVal::Vec(Some(ScVec(VecM::try_from(values).unwrap())))
    }

    #[test]
    fn classifies_ttl_extension_topics() {
        for topic in [
            "extend_contract_data_ttl",
            "contract_instance_ttl_extended",
            "bump_entry",
        ] {
            let kind =
                classify_state_archival_event(&[symbol(topic)], None, SorobanEventType::System);
            assert_eq!(kind, StateArchivalEventKind::TtlExtension, "{topic}");
        }
    }

    #[test]
    fn classifies_archived_and_restored_topics() {
        assert_eq!(
            classify_state_archival_event(
                &[symbol("contract_entry_archived")],
                None,
                SorobanEventType::System
            ),
            StateArchivalEventKind::Archived
        );
        assert_eq!(
            classify_state_archival_event(
                &[symbol("restore_contract_data")],
                None,
                SorobanEventType::System
            ),
            StateArchivalEventKind::Restored
        );
        assert_eq!(
            classify_state_archival_event(
                &[symbol("unarchive_entry")],
                None,
                SorobanEventType::System
            ),
            StateArchivalEventKind::Restored
        );
    }

    #[test]
    fn classifies_nested_topics_and_payload_values() {
        let topics = vec![vec(vec![symbol("ttl"), string("extend")])];
        assert_eq!(
            classify_state_archival_event(&topics, None, SorobanEventType::Contract),
            StateArchivalEventKind::TtlExtension
        );

        let value = ScVal::Map(Some(ScMap(
            VecM::try_from(vec![ScMapEntry {
                key: symbol("state"),
                val: symbol("restored"),
            }])
            .unwrap(),
        )));
        assert_eq!(
            classify_state_archival_event(
                &[symbol("entry")],
                Some(&value),
                SorobanEventType::Contract
            ),
            StateArchivalEventKind::Restored
        );
    }

    #[test]
    fn unrelated_events_are_not_state_archival() {
        assert_eq!(
            classify_state_archival_event(
                &[symbol("transfer"), symbol("mint")],
                None,
                SorobanEventType::Contract
            ),
            StateArchivalEventKind::Other
        );
        assert_eq!(
            classify_state_archival_event(&[], None, SorobanEventType::System),
            StateArchivalEventKind::Other
        );
    }

    #[test]
    fn decoded_event_preserves_metadata_and_classification() {
        let event = ContractEvent::from_decoded(DecodedContractEvent {
            id: "event-1".to_string(),
            paging_token: Some("cursor-1".to_string()),
            ledger: 5_400_000,
            ledger_close_time: Some(1_700_000_000),
            event_type: SorobanEventType::System,
            contract_id: Some(
                "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
            ),
            topics: vec![symbol("extend_contract_instance_ttl")],
            value: Some(symbol("ttl_extended")),
            tx_hash: Some("abc123".to_string()),
            in_successful_contract_call: true,
        });

        assert_eq!(event.id, "event-1");
        assert_eq!(event.paging_token.as_deref(), Some("cursor-1"));
        assert_eq!(event.ledger, 5_400_000);
        assert_eq!(event.ledger_close_time, Some(1_700_000_000));
        assert_eq!(event.event_type, SorobanEventType::System);
        assert_eq!(
            event.topic_symbols,
            vec!["extend_contract_instance_ttl".to_string()]
        );
        assert_eq!(event.value_symbols, vec!["ttl_extended".to_string()]);
        assert_eq!(event.kind, StateArchivalEventKind::TtlExtension);
        assert!(event.is_state_archival());
    }

    #[test]
    fn event_type_parsing_is_tolerant() {
        assert_eq!(
            SorobanEventType::from_rpc_str("SYSTEM"),
            SorobanEventType::System
        );
        assert_eq!(
            SorobanEventType::from_rpc_str("contract"),
            SorobanEventType::Contract
        );
        assert_eq!(
            SorobanEventType::from_rpc_str("diagnostic"),
            SorobanEventType::Diagnostic
        );
        assert_eq!(
            SorobanEventType::from_rpc_str("future-event"),
            SorobanEventType::Unknown
        );
    }
}
