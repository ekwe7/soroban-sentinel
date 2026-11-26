//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod domain;
pub mod error;
pub mod ports;
pub mod usecases;

pub use adapters::*;
pub use domain::*;
pub use error::*;
pub use ports::*;
pub use usecases::*;
