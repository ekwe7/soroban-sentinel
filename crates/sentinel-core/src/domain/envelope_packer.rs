//! Batch transaction envelope split optimizer for Soroban transaction size limits.

use stellar_xdr::curr::{LedgerKey, Limits, ReadXdr, WriteXdr};

use super::StorageEntry;

/// Estimated base transaction envelope overhead in bytes for an ExtendFootprintTTL operation.
///
/// This covers transaction headers (source account, sequence number, fee, memo,
/// preconditions), the ExtendFootprintTTL operation body, Soroban resources metadata
/// (empty read_write footprint, instructions, read/write bytes), and signatures framing.
pub const BASE_ENVELOPE_OVERHEAD_BYTES: usize = 160;

/// Conservative fallback size in bytes for a contract data entry footprint key
/// if neither explicit byte size nor valid XDR base64 is available.
pub const DEFAULT_ENTRY_FOOTPRINT_BYTES: usize = 64;

/// Estimates or measures the footprint byte size of a storage entry in a transaction envelope.
///
/// If the entry's key identifier contains valid base64-encoded XDR for a `LedgerKey`,
/// the exact serialized byte length of the key is measured.
/// If `entry.byte_size() > 0`, that size is used.
/// Otherwise, standard Soroban contract data key footprint overhead is estimated.
pub fn estimate_entry_footprint_bytes(entry: &StorageEntry) -> usize {
    if let Ok(ledger_key) = LedgerKey::from_xdr_base64(entry.key_identifier(), Limits::none()) {
        if let Ok(bytes) = ledger_key.to_xdr(Limits::none()) {
            return bytes.len();
        }
    }

    if entry.byte_size() > 0 {
        entry.byte_size()
    } else {
        DEFAULT_ENTRY_FOOTPRINT_BYTES
    }
}

/// Calculates the total serialized transaction envelope size for a proposed batch of entries.
pub fn calculate_batch_envelope_size(batch: &[StorageEntry]) -> usize {
    if batch.is_empty() {
        return 0;
    }
    let entries_bytes: usize = batch.iter().map(estimate_entry_footprint_bytes).sum();
    BASE_ENVELOPE_OVERHEAD_BYTES.saturating_add(entries_bytes)
}

/// Packs storage entries into minimal transaction batches such that no batch's
/// serialized footprint exceeds `max_envelope_bytes`.
///
/// Uses the Best-Fit Decreasing (BFD) bin-packing heuristic to minimize the
/// total number of transactions required while respecting protocol envelope ceilings.
pub fn pack_extension_batches(
    entries: &[StorageEntry],
    max_envelope_bytes: usize,
) -> Vec<Vec<StorageEntry>> {
    if entries.is_empty() || max_envelope_bytes == 0 {
        return Vec::new();
    }

    // Sort entries descending by footprint size; break ties stably by key identifier.
    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| {
        let size_a = estimate_entry_footprint_bytes(a);
        let size_b = estimate_entry_footprint_bytes(b);
        size_b
            .cmp(&size_a)
            .then_with(|| a.key_identifier().cmp(b.key_identifier()))
    });

    struct BatchBin {
        items: Vec<StorageEntry>,
        current_size: usize,
    }

    let mut bins: Vec<BatchBin> = Vec::new();

    for entry in sorted_entries {
        let entry_size = estimate_entry_footprint_bytes(&entry);

        // Find the best existing bin (one with the least remaining capacity that still fits the entry).
        let mut best_bin_idx = None;
        let mut min_remaining_capacity = usize::MAX;

        for (idx, bin) in bins.iter().enumerate() {
            let candidate_size = bin.current_size.saturating_add(entry_size);
            if candidate_size <= max_envelope_bytes {
                let remaining = max_envelope_bytes.saturating_sub(candidate_size);
                if remaining < min_remaining_capacity {
                    min_remaining_capacity = remaining;
                    best_bin_idx = Some(idx);
                }
            }
        }

        if let Some(idx) = best_bin_idx {
            bins[idx].current_size = bins[idx].current_size.saturating_add(entry_size);
            bins[idx].items.push(entry);
        } else {
            // Must open a new batch bin.
            let initial_size = BASE_ENVELOPE_OVERHEAD_BYTES.saturating_add(entry_size);
            bins.push(BatchBin {
                items: vec![entry],
                current_size: initial_size,
            });
        }
    }

    bins.into_iter().map(|b| b.items).collect()
}
