use async_trait::async_trait;

use crate::error::SentinelResult;
use crate::usecases::TTLReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Slack,
    Discord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertMinLevel {
    Warn,
    Critical,
}

impl AlertMinLevel {
    pub fn includes(self, status: crate::domain::RiskLevel) -> bool {
        let severity = match status {
            crate::domain::RiskLevel::Ok => 0,
            crate::domain::RiskLevel::Warn => 1,
            crate::domain::RiskLevel::Critical | crate::domain::RiskLevel::Archived => 2,
        };
        severity
            >= match self {
                Self::Warn => 1,
                Self::Critical => 2,
            }
    }
}

#[async_trait]
pub trait AlertNotifier: Send + Sync {
    async fn notify(
        &self,
        report: &TTLReport,
        kind: AlertKind,
        min_level: AlertMinLevel,
    ) -> SentinelResult<bool>;
}
