use argon2::{
    Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
    password_hash::{
        Error as PasswordHashError, phc::PasswordHash as PhcPasswordHash, try_generate_salt,
    },
};
use pgrustadmin_app_core::PasswordHash;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordError {
    SaltGeneration,
    InvalidParameters,
    HashingFailed,
    InvalidStoredHash,
    VerificationFailed,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SaltGeneration => "could not generate password salt",
            Self::InvalidParameters => "password hashing parameters are invalid",
            Self::HashingFailed => "could not hash password",
            Self::InvalidStoredHash => "stored password hash is invalid",
            Self::VerificationFailed => "could not verify password",
        };

        formatter.write_str(message)
    }
}

impl Error for PasswordError {}

pub fn hash_password(password: &str) -> Result<PasswordHash, PasswordError> {
    let salt = try_generate_salt().map_err(|_| PasswordError::SaltGeneration)?;
    let argon2 = configured_argon2()?;
    let phc_hash: PhcPasswordHash = argon2
        .hash_password_with_salt(password.as_bytes(), &salt)
        .map_err(|_| PasswordError::HashingFailed)?;

    PasswordHash::from_encoded(phc_hash.to_string()).map_err(|_| PasswordError::HashingFailed)
}

pub fn verify_password(password: &str, stored_hash: &PasswordHash) -> Result<bool, PasswordError> {
    let phc_hash = PhcPasswordHash::new(stored_hash.as_encoded())
        .map_err(|_| PasswordError::InvalidStoredHash)?;

    match configured_argon2()?.verify_password(password.as_bytes(), &phc_hash) {
        Ok(()) => Ok(true),
        Err(PasswordHashError::PasswordInvalid) => Ok(false),
        Err(_) => Err(PasswordError::VerificationFailed),
    }
}

fn configured_argon2() -> Result<Argon2<'static>, PasswordError> {
    let params =
        Params::new(19 * 1024, 2, 1, Some(32)).map_err(|_| PasswordError::InvalidParameters)?;

    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

#[cfg(test)]
mod tests {
    use super::{PasswordError, hash_password, verify_password};
    use pgrustadmin_app_core::PasswordHash;

    #[test]
    fn hashes_with_argon2id_and_a_unique_salt() {
        let first = hash_password("correct horse battery staple").expect("first hash succeeds");
        let second = hash_password("correct horse battery staple").expect("second hash succeeds");

        assert_ne!(first.as_encoded(), second.as_encoded());
        assert!(
            first
                .as_encoded()
                .starts_with("$argon2id$v=19$m=19456,t=2,p=1$")
        );
        assert!(
            verify_password("correct horse battery staple", &first).expect("verification succeeds")
        );
        assert!(!verify_password("wrong password", &first).expect("verification completes"));
    }

    #[test]
    fn rejects_an_invalid_stored_hash() {
        let invalid_hash = PasswordHash::from_encoded("not a PHC hash".to_owned())
            .expect("nonempty stored hash value is accepted by the value type");

        assert_eq!(
            verify_password("password", &invalid_hash),
            Err(PasswordError::InvalidStoredHash)
        );
    }
}
