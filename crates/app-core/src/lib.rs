//! Tipos e regras compartilháveis do PGRustAdmin.

use std::{error::Error, fmt};

use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(Uuid);

impl UserId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for UserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("UserId").field(&self.0).finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EmailAddress(String);

impl EmailAddress {
    pub fn new(value: &str) -> Result<Self, EmailAddressError> {
        let normalized = value.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(EmailAddressError);
        }

        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmailAddressError;

impl fmt::Display for EmailAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("email cannot be empty")
    }
}

impl Error for EmailAddressError {}

pub struct PasswordHash(String);

impl PasswordHash {
    pub fn from_encoded(value: String) -> Result<Self, PasswordHashError> {
        if value.is_empty() {
            return Err(PasswordHashError);
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn as_encoded(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordHashError;

impl fmt::Display for PasswordHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("password hash cannot be empty")
    }
}

impl Error for PasswordHashError {}

pub struct User {
    id: UserId,
    email: EmailAddress,
    password_hash: PasswordHash,
    is_active: bool,
}

impl User {
    #[must_use]
    pub const fn new(
        id: UserId,
        email: EmailAddress,
        password_hash: PasswordHash,
        is_active: bool,
    ) -> Self {
        Self {
            id,
            email,
            password_hash,
            is_active,
        }
    }

    #[must_use]
    pub const fn id(&self) -> UserId {
        self.id
    }

    #[must_use]
    pub const fn email(&self) -> &EmailAddress {
        &self.email
    }

    #[must_use]
    pub const fn password_hash(&self) -> &PasswordHash {
        &self.password_hash
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.is_active
    }
}
