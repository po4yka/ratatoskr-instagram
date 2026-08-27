//! Versioned authenticated encryption for official-account secrets.

use std::collections::BTreeMap;

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead as _, KeyInit as _, Payload},
};
use rand::RngCore as _;
use secrecy::ExposeSecret as _;
use secrecy::SecretString;
use uuid::Uuid;

/// AES-256 key length.
pub const KEY_LEN: usize = 32;
const FORMAT_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 1 + 4 + NONCE_LEN;
const DOMAIN: &[u8] = b"ratatoskr.instagram.credential.v1\0";

/// The semantic kind bound into an encrypted envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Provider access token.
    Access,
    /// Provider refresh token, when the flow supplies one.
    Refresh,
    /// OAuth PKCE verifier retained only while a flow is live.
    PkceVerifier,
}

/// The database subject and semantic kind authenticated with the ciphertext.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBinding {
    /// Account or OAuth-flow identity that owns the material.
    pub subject_id: Uuid,
    /// Meaning of the plaintext.
    pub kind: TokenKind,
}

/// Validated version-to-key mapping with one current write version.
#[derive(Clone)]
pub struct CredentialKeyring {
    current_version: u32,
    keys: BTreeMap<u32, [u8; KEY_LEN]>,
}

impl std::fmt::Debug for CredentialKeyring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialKeyring")
            .field("current_version", &self.current_version)
            .field("keys", &"[REDACTED]")
            .finish()
    }
}

impl CredentialKeyring {
    /// Constructs a keyring when the current write version is present.
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::UnknownKeyVersion`] when it is missing.
    pub fn new(
        current_version: u32,
        keys: BTreeMap<u32, [u8; KEY_LEN]>,
    ) -> Result<Self, CryptoError> {
        if !keys.contains_key(&current_version) {
            return Err(CryptoError::UnknownKeyVersion);
        }
        Ok(Self {
            current_version,
            keys,
        })
    }

    /// Key version selected for new envelopes.
    #[must_use]
    pub const fn current_version(&self) -> u32 {
        self.current_version
    }

    /// Seals a secret using the current key and binding.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`CryptoError`] when sealing is unavailable or fails.
    pub fn seal(
        &self,
        binding: TokenBinding,
        plaintext: &SecretString,
    ) -> Result<Vec<u8>, CryptoError> {
        let key = self
            .keys
            .get(&self.current_version)
            .ok_or(CryptoError::UnknownKeyVersion)?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let mut nonce_bytes = [0_u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let aad = associated_data(binding);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext.expose_secret().as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)?;
        let mut envelope = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        envelope.push(FORMAT_VERSION);
        envelope.extend_from_slice(&self.current_version.to_be_bytes());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    /// Authenticates and opens an envelope using its recorded key version.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`CryptoError`] for malformed, unknown, or unauthentic data.
    pub fn open(
        &self,
        binding: TokenBinding,
        envelope: &[u8],
    ) -> Result<SecretString, CryptoError> {
        if envelope.len() < HEADER_LEN + TAG_LEN {
            return Err(CryptoError::MalformedEnvelope);
        }
        let Some((&format_version, version_and_rest)) = envelope.split_first() else {
            return Err(CryptoError::MalformedEnvelope);
        };
        if format_version != FORMAT_VERSION {
            return Err(CryptoError::MalformedEnvelope);
        }
        let Some((version_bytes, rest)) = version_and_rest.split_first_chunk::<4>() else {
            return Err(CryptoError::MalformedEnvelope);
        };
        let version = u32::from_be_bytes(*version_bytes);
        let key = self
            .keys
            .get(&version)
            .ok_or(CryptoError::UnknownKeyVersion)?;
        let Some((nonce_bytes, ciphertext)) = rest.split_first_chunk::<NONCE_LEN>() else {
            return Err(CryptoError::MalformedEnvelope);
        };
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let aad = associated_data(binding);
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)?;
        let plaintext = String::from_utf8(plaintext).map_err(|_| CryptoError::MalformedEnvelope)?;
        Ok(SecretString::from(plaintext))
    }
}

fn associated_data(binding: TokenBinding) -> Vec<u8> {
    let mut aad = Vec::with_capacity(DOMAIN.len() + 16 + 1);
    aad.extend_from_slice(DOMAIN);
    aad.extend_from_slice(binding.subject_id.as_bytes());
    aad.push(match binding.kind {
        TokenKind::Access => 1,
        TokenKind::Refresh => 2,
        TokenKind::PkceVerifier => 3,
    });
    aad
}

/// Redacted authenticated-envelope failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CryptoError {
    /// The selected key version is not configured.
    #[error("credential key version is unavailable")]
    UnknownKeyVersion,
    /// The stored byte grammar is invalid.
    #[error("credential envelope is malformed")]
    MalformedEnvelope,
    /// Authentication failed due to tampering, substitution, or a wrong key.
    #[error("credential envelope authentication failed")]
    AuthenticationFailed,
    /// RED-phase implementation marker.
    #[error("credential encryption is unsupported")]
    Unsupported,
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret as _;

    use super::*;

    fn keyring() -> CredentialKeyring {
        CredentialKeyring::new(7, BTreeMap::from([(7, [0x42; KEY_LEN])]))
            .expect("the selected test key exists")
    }

    fn binding(kind: TokenKind) -> TokenBinding {
        TokenBinding {
            subject_id: Uuid::from_u128(0x018f_1a2b_3c4d_7e6f_8a9b_0c1d_2e3f_4a5b),
            kind,
        }
    }

    #[test]
    fn token_encryption_round_trip() {
        let keyring = keyring();
        let secret = SecretString::from("IGQVJsynthetic_access_token");
        let sealed = keyring
            .seal(binding(TokenKind::Access), &secret)
            .expect("sealing succeeds");
        assert_ne!(sealed, secret.expose_secret().as_bytes());
        let opened = keyring
            .open(binding(TokenKind::Access), &sealed)
            .expect("opening succeeds");
        assert_eq!(opened.expose_secret(), secret.expose_secret());
    }

    #[test]
    fn equal_plaintexts_use_distinct_nonces() {
        let keyring = keyring();
        let secret = SecretString::from("same-token");
        let first = keyring
            .seal(binding(TokenKind::Access), &secret)
            .expect("first seal succeeds");
        let second = keyring
            .seal(binding(TokenKind::Access), &secret)
            .expect("second seal succeeds");
        assert_ne!(first, second);
    }

    #[test]
    fn wrong_key_version_is_refused() {
        let sealed = keyring()
            .seal(binding(TokenKind::Access), &SecretString::from("secret"))
            .expect("seal succeeds");
        let other = CredentialKeyring::new(8, BTreeMap::from([(8, [0x24; KEY_LEN])]))
            .expect("other key exists");
        assert!(matches!(
            other.open(binding(TokenKind::Access), &sealed),
            Err(CryptoError::UnknownKeyVersion)
        ));
    }

    #[test]
    fn tampered_envelope_is_refused() {
        let keyring = keyring();
        let mut sealed = keyring
            .seal(binding(TokenKind::Access), &SecretString::from("secret"))
            .expect("seal succeeds");
        if let Some(byte) = sealed.last_mut() {
            *byte ^= 1;
        }
        assert!(keyring.open(binding(TokenKind::Access), &sealed).is_err());
    }

    #[test]
    fn account_substitution_is_refused() {
        let keyring = keyring();
        let sealed = keyring
            .seal(binding(TokenKind::Access), &SecretString::from("secret"))
            .expect("seal succeeds");
        let substituted = TokenBinding {
            subject_id: Uuid::now_v7(),
            kind: TokenKind::Access,
        };
        assert!(keyring.open(substituted, &sealed).is_err());
    }

    #[test]
    fn token_kind_substitution_is_refused() {
        let keyring = keyring();
        let sealed = keyring
            .seal(binding(TokenKind::Access), &SecretString::from("secret"))
            .expect("seal succeeds");
        assert!(keyring.open(binding(TokenKind::Refresh), &sealed).is_err());
    }
}
