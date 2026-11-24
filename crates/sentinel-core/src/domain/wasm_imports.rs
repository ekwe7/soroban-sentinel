//! Pure WASM host-function import scanner for Soroban contract bytecode.
//!
//! A Soroban contract imports host functions from the execution environment.
//! This module reads only the WASM type and import sections (no decompilation,
//! no execution) to inventory every import with its module, field and function
//! signature, classify it into a Soroban host subsystem, and flag imports that
//! the environment has retired.
//!
//! Module names and the deprecation table come from `env.json` in
//! `stellar/rs-soroban-env` (commit `0b0f88a9bf435dd8ff188d60bdd9b10bfe14e2c5`).
//! Host modules are single-letter names and host function fields are opaque
//! indices, so classification is per module.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wasmparser::{BinaryReaderError, CompositeInnerType, Parser, Payload, TypeRef, ValType};

use crate::error::SentinelError;

const WASM_MAGIC: [u8; 4] = *b"\0asm";
const WASM_VERSION_1: u32 = 1;
const WASM_HEADER_LEN: usize = 8;

/// A host function the environment no longer supports after a given protocol.
struct DeprecatedHostFunction {
    module: &'static str,
    field: &'static str,
    max_supported_protocol: u32,
}

/// Host functions with a `max_supported_protocol` in `env.json`.
///
/// Extend this table as the environment retires functions. At the pinned
/// commit the only entry is `protocol_gated_dummy` in the test module `t`.
const DEPRECATED_HOST_FUNCTIONS: &[DeprecatedHostFunction] = &[DeprecatedHostFunction {
    module: "t",
    field: "0",
    max_supported_protocol: 19,
}];

/// Standard Soroban host subsystem an import belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSubsystem {
    /// Ledger access (`l`): contract data, code and TTL operations.
    Ledger,
    /// Execution context (`x`): ledger info, logging, timestamps.
    Context,
    /// Cryptography (`c`): hashing, signature verification, curve operations.
    Crypto,
    /// Address and authorization (`a`): `require_auth` and address conversion.
    Auth,
    /// A known Soroban host module outside the four subsystems above
    /// (`i`, `m`, `v`, `b`, `d`, `p`, `t`).
    Other,
    /// A module name that is not a known Soroban host module.
    Unrecognized,
}

impl HostSubsystem {
    /// Classifies a WASM import module name.
    pub fn classify_module(module: &str) -> Self {
        match module {
            "l" => Self::Ledger,
            "x" => Self::Context,
            "c" => Self::Crypto,
            "a" => Self::Auth,
            "i" | "m" | "v" | "b" | "d" | "p" | "t" => Self::Other,
            _ => Self::Unrecognized,
        }
    }

    /// Stable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ledger => "ledger",
            Self::Context => "context",
            Self::Crypto => "crypto",
            Self::Auth => "auth",
            Self::Other => "other",
            Self::Unrecognized => "unrecognized",
        }
    }
}

impl std::fmt::Display for HostSubsystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// WebAssembly value type appearing in an import signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasmValueType {
    /// 32-bit integer.
    I32,
    /// 64-bit integer.
    I64,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
    /// 128-bit vector.
    V128,
    /// Any reference type.
    Ref,
}

impl From<ValType> for WasmValueType {
    fn from(value: ValType) -> Self {
        match value {
            ValType::I32 => Self::I32,
            ValType::I64 => Self::I64,
            ValType::F32 => Self::F32,
            ValType::F64 => Self::F64,
            ValType::V128 => Self::V128,
            ValType::Ref(_) => Self::Ref,
        }
    }
}

/// Parameter and result types of an imported function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmFunctionSignature {
    /// Parameter types in order.
    pub params: Vec<WasmValueType>,
    /// Result types in order.
    pub results: Vec<WasmValueType>,
}

/// Kind of imported item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasmImportKind {
    /// Imported function.
    Function,
    /// Imported table.
    Table,
    /// Imported memory.
    Memory,
    /// Imported global.
    Global,
    /// Imported exception tag.
    Tag,
}

/// One entry of the WASM import section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmImport {
    /// Import module name (a single letter for Soroban host modules).
    pub module: String,
    /// Import field name (an opaque index for Soroban host functions).
    pub field: String,
    /// Kind of imported item.
    pub kind: WasmImportKind,
    /// Function signature, present for function imports.
    pub signature: Option<WasmFunctionSignature>,
    /// Host subsystem inferred from the module name.
    pub subsystem: HostSubsystem,
    /// Last protocol version supporting this host function, when it is retired.
    pub max_supported_protocol: Option<u32>,
}

impl WasmImport {
    /// Returns true when the environment has retired this host function.
    pub fn is_deprecated(&self) -> bool {
        self.max_supported_protocol.is_some()
    }
}

/// Inventory of the host environment a contract WASM depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmEnvironmentProfile {
    /// Every import in section order.
    pub imports: Vec<WasmImport>,
    /// Import counts keyed by module name.
    pub module_counts: BTreeMap<String, usize>,
    /// Import counts keyed by host subsystem.
    pub subsystem_counts: BTreeMap<HostSubsystem, usize>,
    /// Sorted module names that are not known Soroban host modules.
    pub unrecognized_modules: Vec<String>,
}

impl WasmEnvironmentProfile {
    /// Total number of imports.
    pub fn total_imports(&self) -> usize {
        self.imports.len()
    }

    /// Imports belonging to one subsystem.
    pub fn imports_for(&self, subsystem: HostSubsystem) -> impl Iterator<Item = &WasmImport> {
        self.imports
            .iter()
            .filter(move |import| import.subsystem == subsystem)
    }

    /// Imports of host functions the environment has retired.
    pub fn deprecated_imports(&self) -> impl Iterator<Item = &WasmImport> {
        self.imports.iter().filter(|import| import.is_deprecated())
    }

    /// Returns true when any import is retired.
    pub fn has_deprecated_imports(&self) -> bool {
        self.imports.iter().any(WasmImport::is_deprecated)
    }
}

/// Parses the type and import sections of a contract WASM and returns its
/// host environment profile.
///
/// Returns [`SentinelError::WasmAnalysisError`] when the header is truncated,
/// the magic bytes are not `\0asm`, the version is unsupported, or the module
/// is structurally malformed.
pub fn scan_wasm_imports(wasm_bytes: &[u8]) -> Result<WasmEnvironmentProfile, SentinelError> {
    validate_header(wasm_bytes)?;

    let mut types: Vec<Option<WasmFunctionSignature>> = Vec::new();
    let mut imports: Vec<WasmImport> = Vec::new();

    for payload in Parser::new(0).parse_all(wasm_bytes) {
        match payload.map_err(parse_error)? {
            Payload::TypeSection(reader) => {
                for group in reader {
                    let group = group.map_err(parse_error)?;
                    for sub_type in group.into_types() {
                        types.push(match sub_type.composite_type.inner {
                            CompositeInnerType::Func(func) => Some(WasmFunctionSignature {
                                params: func
                                    .params()
                                    .iter()
                                    .copied()
                                    .map(WasmValueType::from)
                                    .collect(),
                                results: func
                                    .results()
                                    .iter()
                                    .copied()
                                    .map(WasmValueType::from)
                                    .collect(),
                            }),
                            _ => None,
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader {
                    let import = import.map_err(parse_error)?;
                    imports.push(build_import(import.module, import.name, import.ty, &types)?);
                }
            }
            _ => {}
        }
    }

    Ok(build_profile(imports))
}

fn validate_header(bytes: &[u8]) -> Result<(), SentinelError> {
    if bytes.len() < WASM_HEADER_LEN {
        return Err(SentinelError::WasmAnalysisError(format!(
            "WASM header truncated: expected at least {WASM_HEADER_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes[0..4] != WASM_MAGIC {
        return Err(SentinelError::WasmAnalysisError(
            "invalid WASM magic bytes: expected \\0asm".to_string(),
        ));
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != WASM_VERSION_1 {
        return Err(SentinelError::WasmAnalysisError(format!(
            "unsupported WASM version {version}: expected {WASM_VERSION_1}"
        )));
    }
    Ok(())
}

fn parse_error(error: BinaryReaderError) -> SentinelError {
    SentinelError::WasmAnalysisError(format!("malformed WASM module: {error}"))
}

fn build_import(
    module: &str,
    field: &str,
    ty: TypeRef,
    types: &[Option<WasmFunctionSignature>],
) -> Result<WasmImport, SentinelError> {
    let (kind, signature) = match ty {
        TypeRef::Func(index) => {
            let signature = types
                .get(index as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    SentinelError::WasmAnalysisError(format!(
                        "import {module}.{field} references type index {index}, which is not a function type"
                    ))
                })?;
            (WasmImportKind::Function, Some(signature.clone()))
        }
        TypeRef::Table(_) => (WasmImportKind::Table, None),
        TypeRef::Memory(_) => (WasmImportKind::Memory, None),
        TypeRef::Global(_) => (WasmImportKind::Global, None),
        TypeRef::Tag(_) => (WasmImportKind::Tag, None),
    };

    let max_supported_protocol = if kind == WasmImportKind::Function {
        DEPRECATED_HOST_FUNCTIONS
            .iter()
            .find(|entry| entry.module == module && entry.field == field)
            .map(|entry| entry.max_supported_protocol)
    } else {
        None
    };

    Ok(WasmImport {
        module: module.to_string(),
        field: field.to_string(),
        kind,
        signature,
        subsystem: HostSubsystem::classify_module(module),
        max_supported_protocol,
    })
}

fn build_profile(imports: Vec<WasmImport>) -> WasmEnvironmentProfile {
    let mut module_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut subsystem_counts: BTreeMap<HostSubsystem, usize> = BTreeMap::new();
    let mut unrecognized: BTreeSet<String> = BTreeSet::new();

    for import in &imports {
        *module_counts.entry(import.module.clone()).or_insert(0) += 1;
        *subsystem_counts.entry(import.subsystem).or_insert(0) += 1;
        if import.subsystem == HostSubsystem::Unrecognized {
            unrecognized.insert(import.module.clone());
        }
    }

    WasmEnvironmentProfile {
        imports,
        module_counts,
        subsystem_counts,
        unrecognized_modules: unrecognized.into_iter().collect(),
    }
}
