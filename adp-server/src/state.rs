use std::sync::Arc;

use nostr_sdk::Keys;

use crate::config::Config;
use crate::relay::RelayClient;
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Storage,
    pub relay: Arc<RelayClient>,
    /// The DEDICATED fulfillment key this server signs receipts with. See
    /// ADP-01.md "Security Considerations" — this must never be a developer's
    /// primary identity key.
    pub fulfillment_keys: Keys,
}
