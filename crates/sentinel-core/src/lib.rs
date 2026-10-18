//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

pub mod domain;
pub mod error;

pub use domain::{Durability, EntryType, StorageEntry};
pub use error::{SentinelError, SentinelResult};
