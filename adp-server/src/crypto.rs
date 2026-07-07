use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("invalid master key length: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
    #[error("decryption failed - invalid key or corrupted data")]
    DecryptionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedSecret {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

pub struct MasterCipher {
    cipher: Aes256Gcm,
}

impl MasterCipher {
    pub fn new(master_key: &[u8]) -> Result<Self, CryptoError> {
        if master_key.len() != 32 {
            return Err(CryptoError::InvalidKeyLength(master_key.len()));
        }
        let cipher = Aes256Gcm::new_from_slice(master_key)
            .map_err(|_| CryptoError::InvalidKeyLength(master_key.len()))?;
        Ok(Self { cipher })
    }

    pub fn encrypt(&self, data: &[u8]) -> EncryptedSecret {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, data)
            .expect("encryption should not fail with valid inputs");
        EncryptedSecret {
            nonce: nonce.to_vec(),
            ciphertext,
        }
    }

    pub fn decrypt(&self, data: &EncryptedSecret) -> Result<Vec<u8>, CryptoError> {
        let nonce = Nonce::from_slice(&data.nonce);
        self.cipher
            .decrypt(nonce, data.ciphertext.as_ref())
            .map_err(|_| CryptoError::DecryptionFailed)
    }
}
