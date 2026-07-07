use std::sync::Arc;

use nostr_sdk::Keys;

use crate::config::Config;
use crate::keystore::KeyStore;
use crate::relay::Relay;
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Storage,
    pub relay: Arc<dyn Relay>,
    pub keystore: Arc<KeyStore>,
    pub operator_keys: Keys,
}
