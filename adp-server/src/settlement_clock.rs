//! Injectable settlement time for deterministic, test-owned routers. The normal
//! router always uses wall time; no environment variable or HTTP clock control.
use std::sync::Arc;

#[derive(Clone)]
pub struct SettlementClock(Arc<dyn Fn() -> Option<u64> + Send + Sync>);

impl SettlementClock {
    pub fn new(now: impl Fn() -> Option<u64> + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }
    pub fn now_secs(&self) -> Result<u64, crate::error::ApiError> {
        (self.0)()
            .ok_or_else(|| crate::error::ApiError::Internal("invalid settlement clock".into()))
    }
}

impl Default for SettlementClock {
    fn default() -> Self {
        Self::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        })
    }
}
