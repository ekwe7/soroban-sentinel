//! Pure data-entry rent comparisons using the protocol's byte-ledger formula.
//! Reference: https://github.com/stellar/rs-soroban-env/blob/main/soroban-env-host/src/fees.rs
//! Caller supplies network rates and fee-billed entry sizes. Inclusion, resource,
//! restoration, and TTL-write fees require transaction-specific estimates.

use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Durability, NetworkLimits};

#[derive(Debug, Clone, Copy)]
pub struct RentRates {
    pub fee_per_rent_kib_stroops: u64,
    pub persistent_denominator: NonZeroU64,
    pub temporary_denominator: NonZeroU64,
    /// Calendar-year ledger estimate at the caller's assumed close velocity.
    pub ledgers_per_year: NonZeroU32,
    /// Protocol-specific code rent divisor, applied after whole-stroop rounding.
    pub code_rent_discount: NonZeroU32,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RentAnalysisError {
    #[error("migration requires distinct Persistent and Temporary durabilities")]
    InvalidTransition,
    #[error("TTL horizon and network minimum TTLs must be positive and at most max_entry_ttl")]
    InvalidHorizon,
    #[error("rent aggregation exceeds u128 or entry size exceeds u32")]
    Overflow,
}

impl RentRates {
    /// ceil(bytes * fee_per_rent_KiB * ledgers / (1024 * durability_denominator)).
    /// Instance and code use the persistent rate. Callers supply fee-billed
    /// sizes and the code discount appropriate to the modeled protocol.
    pub fn entry_rent(
        &self,
        bytes: u32,
        ledgers: u32,
        durability: Durability,
    ) -> Result<u128, RentAnalysisError> {
        let denominator = match durability {
            Durability::Persistent | Durability::Instance | Durability::Code => {
                self.persistent_denominator.get()
            }
            Durability::Temporary => self.temporary_denominator.get(),
        };
        let rent =
            (u128::from(bytes) * u128::from(self.fee_per_rent_kib_stroops) * u128::from(ledgers))
                .div_ceil(1024 * u128::from(denominator));
        Ok(if durability == Durability::Code {
            rent.div_ceil(u128::from(self.code_rent_discount.get()))
        } else {
            rent
        })
    }

    fn annual_rent(
        &self,
        bytes: u32,
        horizon: u32,
        durability: Durability,
    ) -> Result<u128, RentAnalysisError> {
        let year = self.ledgers_per_year.get();
        Ok(
            self.entry_rent(bytes, horizon, durability)? * u128::from(year / horizon)
                + self.entry_rent(bytes, year % horizon, durability)?,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MigrationRequest {
    pub byte_size: u32,
    pub from: Durability,
    pub to: Durability,
    /// Future renewal horizon, excluding the already-paid current ledger.
    pub horizon_ledgers: u32,
    pub replaceable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurabilityMigrationAnalysis {
    pub from: Durability,
    pub to: Durability,
    pub byte_size: u32,
    pub source_horizon_ledgers: u32,
    pub target_horizon_ledgers: u32,
    pub max_entry_ttl: u32,
    pub max_extension_ledgers: u32,
    pub source_annual_rent_stroops: u128,
    pub target_annual_rent_stroops: u128,
    /// Target minus source. Negative means savings.
    pub annual_rent_delta_stroops: i128,
    pub annual_rent_delta_xlm: f64,
    pub warnings: Vec<String>,
}

impl DurabilityMigrationAnalysis {
    pub fn analyze(
        request: MigrationRequest,
        rates: &RentRates,
        limits: &NetworkLimits,
    ) -> Result<Self, RentAnalysisError> {
        if !matches!(
            (request.from, request.to),
            (Durability::Persistent, Durability::Temporary)
                | (Durability::Temporary, Durability::Persistent)
        ) {
            return Err(RentAnalysisError::InvalidTransition);
        }
        if request.horizon_ledgers == 0
            || limits.max_entry_ttl <= 1
            || limits.min_persistent_ttl == 0
            || limits.min_temporary_ttl == 0
            || limits.min_persistent_ttl > limits.max_entry_ttl
            || limits.min_temporary_ttl > limits.max_entry_ttl
        {
            return Err(RentAnalysisError::InvalidHorizon);
        }
        // maxEntryTTL includes the current ledger; future extension is capped
        // at maxEntryTTL - 1. New-entry minimum lifetimes are inclusive too.
        let max_extension_ledgers = limits.max_entry_ttl - 1;
        let horizon = |durability| {
            request.horizon_ledgers.clamp(
                if durability == Durability::Temporary {
                    limits.min_temporary_ttl.saturating_sub(1).max(1)
                } else {
                    limits.min_persistent_ttl.saturating_sub(1).max(1)
                },
                max_extension_ledgers,
            )
        };
        let source_horizon_ledgers = horizon(request.from);
        let target_horizon_ledgers = horizon(request.to);
        let source = rates.annual_rent(request.byte_size, source_horizon_ledgers, request.from)?;
        let target = rates.annual_rent(request.byte_size, target_horizon_ledgers, request.to)?;
        let delta = i128::try_from(target).map_err(|_| RentAnalysisError::Overflow)?
            - i128::try_from(source).map_err(|_| RentAnalysisError::Overflow)?;
        let mut warnings = Vec::new();
        if request.horizon_ledgers > max_extension_ledgers {
            warnings.push("Requested horizon exceeds network max_entry_ttl; renew before each capped horizon expires.".into());
        }
        if target_horizon_ledgers > request.horizon_ledgers {
            warnings.push("The target minimum TTL exceeds the requested horizon.".into());
        }
        if request.to == Durability::Temporary {
            warnings.push(
                "Temporary entries are permanently evicted at expiry and cannot be restored."
                    .into(),
            );
            if !request.replaceable {
                warnings.push("Data loss risk: non-replaceable data must be recoverable elsewhere before temporary placement.".into());
            }
        }
        Ok(Self {
            from: request.from,
            to: request.to,
            byte_size: request.byte_size,
            source_horizon_ledgers,
            target_horizon_ledgers,
            max_entry_ttl: limits.max_entry_ttl,
            max_extension_ledgers,
            source_annual_rent_stroops: source,
            target_annual_rent_stroops: target,
            annual_rent_delta_stroops: delta,
            annual_rent_delta_xlm: delta as f64 / 10_000_000.0,
            warnings,
        })
    }
}
