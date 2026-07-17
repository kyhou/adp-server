use nostr::EventId;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChainResolutionError {
    #[error("chain is empty")]
    Empty,
    #[error("missing predecessor {0}")]
    MissingPredecessor(EventId),
    #[error("chain has multiple roots")]
    MultipleRoots,
    #[error("chain forks at {0}")]
    Fork(EventId),
    #[error("chain contains a cycle")]
    Cycle,
    #[error("chain contains disconnected events")]
    Disconnected,
    #[error("chain invariant changed: {0}")]
    InvariantMutation(&'static str),
    #[error("invalid chain transition: {0}")]
    InvalidTransition(String),
    #[error("successor timestamp does not increase")]
    TimestampRegression,
    #[error("chain retrieval limit exceeded")]
    LimitExceeded,
}
