//! Password generation and the explicit "no password" value.
//!
//! `F-CRD-01` requires a 24-character password from the OS CSPRNG, and
//! `F-CRD-03` makes "no password" a first-class mode. [`generate_password`]
//! and [`InstanceSecret`] are the only two places that answer those questions,
//! so no other module has to special-case [`AuthMode::None`].

use zeroize::Zeroize;

use crate::DblError;
use crate::consts;
use crate::error::Result;
use crate::model::AuthMode;
use crate::secret::SecretString;

/// Characters a generated password is drawn from.
///
/// Digits and ASCII letters only. The value is therefore safe unquoted in a
/// POSIX shell, in a `.env` block, inside an engine config file and inside a
/// connection URI; nothing needs escaping. The alphabet deliberately excludes
/// whitespace, quotes, `$`, `\`, `%`, `&`, `#`, `?`, `:` and `@`, each of which
/// is special to at least one of those contexts.
pub const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Generates a superuser password of [`consts::PASSWORD_LEN`] characters.
///
/// Bytes come straight from the operating system CSPRNG, and each character is
/// chosen by rejection sampling, so every symbol in [`ALPHABET`] is equally
/// likely: there is no modulo bias.
pub fn generate_password() -> Result<SecretString> {
    generate_with(&mut rand::rngs::SysRng, consts::PASSWORD_LEN)
}

/// Generates a password of `len` characters from `rng`.
///
/// Split out from [`generate_password`] so tests can drive it with a seeded
/// generator and assert the distribution without touching the OS CSPRNG.
pub(crate) fn generate_with<R: rand::TryRng>(rng: &mut R, len: usize) -> Result<SecretString> {
    if len == 0 {
        return Err(DblError::InvalidInput {
            field: "password_len",
            reason: "must be at least one character".to_owned(),
        });
    }

    let modulus = ALPHABET.len() as u8;
    // 62 is not a power of two, so sampling a masked byte and rejecting values
    // at or above 62 keeps every character exactly equally likely.
    let mask = modulus.next_power_of_two() - 1;
    let mut out = String::with_capacity(len);

    for _ in 0..len {
        let index = loop {
            let drawn = match rng.try_next_u32() {
                Ok(value) => (value as u8) & mask,
                Err(_) => {
                    out.zeroize();
                    return Err(random_source_failed());
                }
            };
            if drawn < modulus {
                break drawn;
            }
        };
        out.push(ALPHABET[index as usize] as char);
    }

    Ok(SecretString::new(out))
}

fn random_source_failed() -> DblError {
    DblError::Secret {
        reason: "the operating system random number generator failed".to_owned(),
    }
}

/// The credential an instance was created with, including "none".
///
/// This exists so callers never have to ask "is auth `None`?" before touching a
/// password. A [`InstanceSecret::NoPassword`] carries no buffer at all, and the
/// password variant redacts itself everywhere, so the value is safe to log.
#[derive(Clone, PartialEq, Eq)]
pub enum InstanceSecret {
    /// A generated or user-supplied password.
    Password(SecretString),
    /// Trust authentication (`F-CRD-03`): no credential exists.
    NoPassword,
}

impl InstanceSecret {
    /// Wraps an existing password.
    pub fn password(secret: SecretString) -> Self {
        Self::Password(secret)
    }

    /// Generates a fresh password, the default for a new instance.
    pub fn generate() -> Result<Self> {
        generate_password().map(Self::Password)
    }

    /// The value for the "no password" mode (`F-CRD-03`).
    pub const fn none() -> Self {
        Self::NoPassword
    }

    /// Builds the value that matches `auth`, rejecting inconsistent pairs.
    ///
    /// A password mode without a password, or a no-auth mode that carries one,
    /// is a programming error at a boundary and is reported instead of being
    /// silently dropped.
    pub fn for_mode(auth: AuthMode, secret: Option<SecretString>) -> Result<Self> {
        match (auth.requires_password(), secret) {
            (true, Some(secret)) => Ok(Self::Password(secret)),
            (true, None) => Err(DblError::InvalidInput {
                field: "password",
                reason: "this authentication mode requires a password".to_owned(),
            }),
            (false, None) => Ok(Self::NoPassword),
            (false, Some(_)) => Err(DblError::InvalidInput {
                field: "password",
                reason: "no-password mode must not carry a password".to_owned(),
            }),
        }
    }

    /// The password, or `None` in no-auth mode.
    pub const fn as_password(&self) -> Option<&SecretString> {
        match self {
            Self::Password(secret) => Some(secret),
            Self::NoPassword => None,
        }
    }

    /// Whether a password is carried.
    pub const fn is_password(&self) -> bool {
        matches!(self, Self::Password(_))
    }
}

impl std::fmt::Debug for InstanceSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password(_) => f.write_str("InstanceSecret::Password(***)"),
            Self::NoPassword => f.write_str("InstanceSecret::NoPassword"),
        }
    }
}

impl std::fmt::Display for InstanceSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password(_) => f.write_str("***"),
            Self::NoPassword => f.write_str("no-password"),
        }
    }
}

#[cfg(test)]
#[path = "password/tests.rs"]
mod tests;
