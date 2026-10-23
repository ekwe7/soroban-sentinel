use thiserror::Error;

/// Unified domain and infrastructure error types for Soroban Sentinel.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SentinelError {
    #[error("Invalid contract address: {0}")]
    InvalidContractAddress(String),

    #[error("Invalid XDR key: {0}")]
    InvalidXdrKey(String),

    #[error("RPC connection timeout: {0}")]
    RpcConnectionTimeout(String),

    #[error("RPC response error (code: {code}): {message}")]
    RpcResponseError { code: i64, message: String },

    #[error("Contract instance not found: {0}")]
    ContractInstanceNotFound(String),

    #[error("Code entry not found: {0}")]
    CodeEntryNotFound(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Invalid threshold configuration: {0}")]
    InvalidThresholdConfig(String),

    #[error("Invalid RPC URL: {0}")]
    InvalidRpcUrl(String),
}

/// Convenience type alias for `Result<T, SentinelError>`.
pub type SentinelResult<T> = Result<T, SentinelError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_formatting() {
        let err = SentinelError::InvalidContractAddress("C123".to_string());
        assert_eq!(err.to_string(), "Invalid contract address: C123");

        let err = SentinelError::InvalidXdrKey("bad_xdr".to_string());
        assert_eq!(err.to_string(), "Invalid XDR key: bad_xdr");

        let err = SentinelError::RpcConnectionTimeout("https://rpc.example.com".to_string());
        assert_eq!(
            err.to_string(),
            "RPC connection timeout: https://rpc.example.com"
        );

        let err = SentinelError::RpcResponseError {
            code: -32600,
            message: "Invalid Request".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "RPC response error (code: -32600): Invalid Request"
        );

        let err = SentinelError::ContractInstanceNotFound("CAAA...".to_string());
        assert_eq!(err.to_string(), "Contract instance not found: CAAA...");

        let err = SentinelError::CodeEntryNotFound("hash123".to_string());
        assert_eq!(err.to_string(), "Code entry not found: hash123");

        let err = SentinelError::SerializationError("JSON syntax error".to_string());
        assert_eq!(err.to_string(), "Serialization error: JSON syntax error");
    }

    #[test]
    fn test_sentinel_result_alias() {
        fn return_ok() -> SentinelResult<u32> {
            Ok(42)
        }

        fn return_err() -> SentinelResult<u32> {
            Err(SentinelError::InvalidContractAddress("test".to_string()))
        }

        assert_eq!(return_ok().unwrap(), 42);
        assert!(return_err().is_err());
    }
}
