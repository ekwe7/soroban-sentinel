//! Pure domain mathematics for Soroban storage TTL decay and time horizon estimation.

/// Target ledger close time in seconds on Stellar/Soroban (approximately 5 seconds per ledger).
pub const TARGET_LEDGER_CLOSE_SECONDS: f64 = 5.0;

/// Number of seconds in a standard day (24 * 60 * 60).
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// Calculates remaining ledgers until entry expiration.
///
/// # Special Cases:
/// - If `live_until == 0`, the entry is uninitialized or already archived, returning `0`.
/// - If `latest_ledger > live_until`, the entry has expired, returning a negative count.
pub fn calculate_remaining_ledgers(live_until: u32, latest_ledger: u32) -> i64 {
    if live_until == 0 {
        return 0;
    }
    (live_until as i64) - (latest_ledger as i64)
}

/// Estimates remaining wall-clock seconds until expiration based on target ledger close speed.
///
/// Returns `0.0` if `remaining_ledgers <= 0`.
pub fn estimate_remaining_seconds(remaining_ledgers: i64) -> f64 {
    if remaining_ledgers <= 0 {
        return 0.0;
    }
    (remaining_ledgers as f64) * TARGET_LEDGER_CLOSE_SECONDS
}

/// Estimates remaining wall-clock days given remaining seconds.
///
/// Returns `0.0` if `remaining_seconds <= 0.0`.
pub fn estimate_remaining_days(remaining_seconds: f64) -> f64 {
    if remaining_seconds <= 0.0 {
        return 0.0;
    }
    remaining_seconds / SECONDS_PER_DAY
}

/// Calculates estimated remaining horizon in days directly from ledger sequence boundaries.
pub fn estimate_horizon_days(live_until: u32, latest_ledger: u32) -> f64 {
    let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
    let remaining_seconds = estimate_remaining_seconds(remaining_ledgers);
    estimate_remaining_days(remaining_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positive_remaining_ledgers() {
        let live_until = 5_500_000;
        let latest = 5_400_000;

        let ledgers = calculate_remaining_ledgers(live_until, latest);
        assert_eq!(ledgers, 100_000);

        let seconds = estimate_remaining_seconds(ledgers);
        assert_eq!(seconds, 500_000.0);

        let days = estimate_remaining_days(seconds);
        // 500,000 / 86,400 ~= 5.787037...
        assert!((days - 5.787037).abs() < 1e-4);

        let direct_days = estimate_horizon_days(live_until, latest);
        assert_eq!(days, direct_days);
    }

    #[test]
    fn test_expired_entry_negative_ledgers() {
        let live_until = 5_399_990;
        let latest = 5_400_000;

        let ledgers = calculate_remaining_ledgers(live_until, latest);
        assert_eq!(ledgers, -10);

        let seconds = estimate_remaining_seconds(ledgers);
        assert_eq!(seconds, 0.0);

        let days = estimate_remaining_days(seconds);
        assert_eq!(days, 0.0);
    }

    #[test]
    fn test_exact_boundary_zero_remaining() {
        let live_until = 5_400_000;
        let latest = 5_400_000;

        let ledgers = calculate_remaining_ledgers(live_until, latest);
        assert_eq!(ledgers, 0);

        let seconds = estimate_remaining_seconds(ledgers);
        assert_eq!(seconds, 0.0);

        let days = estimate_remaining_days(seconds);
        assert_eq!(days, 0.0);
    }

    #[test]
    fn test_archived_or_uninitialized_zero() {
        let live_until = 0;
        let latest = 5_400_000;

        let ledgers = calculate_remaining_ledgers(live_until, latest);
        assert_eq!(ledgers, 0);

        let seconds = estimate_remaining_seconds(ledgers);
        assert_eq!(seconds, 0.0);

        let days = estimate_remaining_days(seconds);
        assert_eq!(days, 0.0);
    }
}
