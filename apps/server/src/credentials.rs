use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, KeyInit, Payload},
};
use std::{error::Error, fmt};
use uuid::Uuid;
use zeroize::Zeroizing;

const KEY_ENV: &str = "PGRUSTADMIN_CREDENTIAL_KEY";
const ENVELOPE_VERSION: u8 = 1;
const NONCE_LENGTH: usize = 24;
const TAG_LENGTH: usize = 16;
const AAD_PREFIX: &[u8] = b"pgrustadmin/connection-credential/v1\0";

pub struct CredentialCipher {
    cipher: XChaCha20Poly1305,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    MissingKey,
    InvalidKey,
    InvalidEnvelope,
    UnsupportedVersion,
    AuthenticationFailed,
    EncryptionFailed,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingKey => "credential encryption key is not configured",
            Self::InvalidKey => "credential encryption key is invalid",
            Self::InvalidEnvelope => "credential envelope is invalid",
            Self::UnsupportedVersion => "credential envelope version is unsupported",
            Self::AuthenticationFailed => "credential authentication failed",
            Self::EncryptionFailed => "credential encryption failed",
        };

        formatter.write_str(message)
    }
}

impl Error for CredentialError {}

impl CredentialCipher {
    pub fn from_env() -> Result<Self, CredentialError> {
        let encoded_key = std::env::var(KEY_ENV).map_err(|error| match error {
            std::env::VarError::NotPresent => CredentialError::MissingKey,
            std::env::VarError::NotUnicode(_) => CredentialError::InvalidKey,
        })?;
        let encoded_key = Zeroizing::new(encoded_key);
        let decoded_key = decode_key(&encoded_key)?;
        Self::from_key_bytes(&decoded_key)
    }

    fn from_key_bytes(key: &[u8]) -> Result<Self, CredentialError> {
        let key: [u8; 32] = key.try_into().map_err(|_| CredentialError::InvalidKey)?;
        let key = Zeroizing::new(key);
        let cipher =
            XChaCha20Poly1305::new_from_slice(&*key).map_err(|_| CredentialError::InvalidKey)?;

        Ok(Self { cipher })
    }

    pub fn encrypt(
        &self,
        plaintext: &[u8],
        owner_id: Uuid,
        connection_id: Uuid,
    ) -> Result<Vec<u8>, CredentialError> {
        let nonce = XNonce::try_generate().map_err(|_| CredentialError::EncryptionFailed)?;
        let aad = associated_data(owner_id, connection_id);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CredentialError::EncryptionFailed)?;

        let mut envelope = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
        envelope.push(ENVELOPE_VERSION);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);

        Ok(envelope)
    }

    pub fn decrypt(
        &self,
        envelope: &[u8],
        owner_id: Uuid,
        connection_id: Uuid,
    ) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        let Some(&version) = envelope.first() else {
            return Err(CredentialError::InvalidEnvelope);
        };
        if version != ENVELOPE_VERSION {
            return Err(CredentialError::UnsupportedVersion);
        }
        if envelope.len() < 1 + NONCE_LENGTH + TAG_LENGTH {
            return Err(CredentialError::InvalidEnvelope);
        }

        let nonce_bytes: [u8; NONCE_LENGTH] = envelope[1..1 + NONCE_LENGTH]
            .try_into()
            .map_err(|_| CredentialError::InvalidEnvelope)?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = associated_data(owner_id, connection_id);

        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &envelope[1 + NONCE_LENGTH..],
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| CredentialError::AuthenticationFailed)
    }
}

fn associated_data(owner_id: Uuid, connection_id: Uuid) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_PREFIX.len() + 32);
    aad.extend_from_slice(AAD_PREFIX);
    aad.extend_from_slice(owner_id.as_bytes());
    aad.extend_from_slice(connection_id.as_bytes());
    aad
}

fn decode_key(encoded_key: &str) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
    let decoded_key = STANDARD
        .decode(encoded_key.as_bytes())
        .map_err(|_| CredentialError::InvalidKey)?;
    if decoded_key.len() != 32 {
        return Err(CredentialError::InvalidKey);
    }

    Ok(Zeroizing::new(decoded_key))
}

#[cfg(test)]
mod tests {
    use super::{CredentialCipher, CredentialError, ENVELOPE_VERSION, decode_key};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use uuid::Uuid;

    fn cipher() -> CredentialCipher {
        CredentialCipher::from_key_bytes(&[42; 32]).expect("test key is valid")
    }

    #[test]
    fn encrypts_and_decrypts_with_the_same_owner_and_connection() {
        let cipher = cipher();
        let owner_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let plaintext = b"database-password";

        let envelope = cipher
            .encrypt(plaintext, owner_id, connection_id)
            .expect("credential encryption succeeds");
        let second_envelope = cipher
            .encrypt(plaintext, owner_id, connection_id)
            .expect("second credential encryption succeeds");

        assert_eq!(envelope[0], ENVELOPE_VERSION);
        assert_ne!(&envelope[1 + 24..], plaintext);
        assert_ne!(&envelope[1..25], &second_envelope[1..25]);
        let decrypted = cipher
            .decrypt(&envelope, owner_id, connection_id)
            .expect("credential decryption succeeds");
        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[test]
    fn rejects_tampering_wrong_key_and_changed_owner_or_connection() {
        let cipher = cipher();
        let owner_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let envelope = cipher
            .encrypt(b"database-password", owner_id, connection_id)
            .expect("credential encryption succeeds");
        let mut tampered = envelope.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;

        assert_eq!(
            cipher.decrypt(&tampered, owner_id, connection_id),
            Err(CredentialError::AuthenticationFailed)
        );
        assert_eq!(
            cipher.decrypt(&envelope, Uuid::new_v4(), connection_id),
            Err(CredentialError::AuthenticationFailed)
        );
        assert_eq!(
            cipher.decrypt(&envelope, owner_id, Uuid::new_v4()),
            Err(CredentialError::AuthenticationFailed)
        );
        assert_eq!(
            CredentialCipher::from_key_bytes(&[7; 32])
                .expect("second test key is valid")
                .decrypt(&envelope, owner_id, connection_id),
            Err(CredentialError::AuthenticationFailed)
        );
    }

    #[test]
    fn rejects_truncated_and_unknown_version_envelopes() {
        let cipher = cipher();

        assert_eq!(
            cipher.decrypt(&[], Uuid::new_v4(), Uuid::new_v4()),
            Err(CredentialError::InvalidEnvelope)
        );
        assert_eq!(
            cipher.decrypt(&[2, 0, 0], Uuid::new_v4(), Uuid::new_v4()),
            Err(CredentialError::UnsupportedVersion)
        );
        assert_eq!(
            cipher.decrypt(&[ENVELOPE_VERSION, 0, 0], Uuid::new_v4(), Uuid::new_v4()),
            Err(CredentialError::InvalidEnvelope)
        );
    }

    #[test]
    fn rejects_keys_that_are_not_32_bytes() {
        assert_eq!(
            CredentialCipher::from_key_bytes(&[0; 31]).err(),
            Some(CredentialError::InvalidKey)
        );
    }

    #[test]
    fn decodes_only_base64_keys_with_32_bytes() {
        assert_eq!(
            decode_key("not base64").err(),
            Some(CredentialError::InvalidKey)
        );

        let short_key = STANDARD.encode([1; 31]);
        assert_eq!(
            decode_key(&short_key).err(),
            Some(CredentialError::InvalidKey)
        );

        let valid_key = STANDARD.encode([1; 32]);
        assert_eq!(
            decode_key(&valid_key).expect("32-byte key decodes").len(),
            32
        );
    }
}
