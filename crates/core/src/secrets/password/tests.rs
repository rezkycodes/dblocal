use std::collections::BTreeSet;

use rand::SeedableRng;
use rand::rngs::SmallRng;

use super::{ALPHABET, InstanceSecret, generate_password, generate_with, random_source_failed};
use crate::consts;
use crate::error::{CODE_INVALID_INPUT, CODE_SECRET};
use crate::model::AuthMode;
use crate::secret::SecretString;

const CANARY: &str = "hunter2-password-canary";

fn allowed() -> BTreeSet<char> {
    ALPHABET.iter().map(|b| *b as char).collect()
}

#[test]
fn generates_default_length() {
    let secret = generate_password().expect("os rng should work");
    assert_eq!(secret.len(), consts::PASSWORD_LEN);
    assert_eq!(secret.expose().len(), consts::PASSWORD_LEN);
    assert_eq!(consts::PASSWORD_LEN, 24);
}

#[test]
fn uses_only_shell_and_config_safe_characters() {
    let forbidden = [
        ' ', '\'', '"', '$', '\\', '%', '&', '#', '?', '!', ':', '@', '/', '\n', '\r', '\t', '=',
        '+', '<', '>', '|', ';', '`',
    ];
    let allowed = allowed();
    for _ in 0..40 {
        let secret = generate_password().expect("os rng should work");
        for ch in secret.expose().chars() {
            assert!(ch.is_ascii(), "non-ascii: {ch:?}");
            assert!(allowed.contains(&ch), "unexpected character: {ch:?}");
            assert!(
                !forbidden.contains(&ch),
                "forbidden character leaked: {ch:?}"
            );
        }
    }
}

#[test]
fn rejects_zero_length() {
    let mut rng = SmallRng::seed_from_u64(1);
    let err = generate_with(&mut rng, 0).expect_err("zero length must fail");
    assert_eq!(err.code(), CODE_INVALID_INPUT);
}

#[test]
fn chi_square_style_sanity_over_many_samples() {
    // Deterministic, unbiased RNG: draw a large sample and check that no
    // character is wildly under- or over-represented. The alphabet is 62
    // symbols and we draw 62 * 3000 characters in total, so the expected
    // count per symbol is 3000. A +-40% band catches a broken index/mapping
    // or a biased reject rule while staying far from flaky.
    let mut rng = SmallRng::seed_from_u64(0x5eed_cafe);
    let draws_per_symbol = 3000usize;
    let total = ALPHABET.len() * draws_per_symbol;
    let mut counts = [0usize; 62];
    let generated = generate_with(&mut rng, total).expect("seeded rng");
    for ch in generated.expose().chars() {
        let index = ALPHABET
            .iter()
            .position(|b| *b == ch as u8)
            .expect("character must come from the alphabet");
        counts[index] += 1;
    }

    let expected = draws_per_symbol as f64;
    let min = (expected * 0.6) as usize;
    let max = (expected * 1.4) as usize;
    for (i, count) in counts.iter().enumerate() {
        assert!(
            (*count >= min) && (*count <= max),
            "symbol {} count {} outside [{min}, {max}] (expected ~{expected})",
            ALPHABET[i] as char,
            count
        );
    }
}

#[test]
fn passwords_vary_across_calls() {
    let a = generate_password()
        .expect("os rng should work")
        .expose()
        .to_owned();
    let b = generate_password()
        .expect("os rng should work")
        .expose()
        .to_owned();
    assert_ne!(a, b, "two consecutive OS-generated passwords should differ");
}

#[test]
fn instance_secret_helpers_cover_no_password_mode() {
    let none = InstanceSecret::none();
    assert!(!none.is_password());
    assert!(none.as_password().is_none());
    assert_eq!(format!("{none}"), "no-password");
    assert_eq!(format!("{none:?}"), "InstanceSecret::NoPassword");

    let generated = InstanceSecret::generate().expect("generate");
    assert!(generated.is_password());
    assert!(generated.as_password().is_some());

    let explicit = InstanceSecret::password(SecretString::from(CANARY));
    let rendered = format!("{explicit:?} {explicit}");
    assert!(!rendered.contains(CANARY));
    assert!(rendered.contains("***"));
}

#[test]
fn for_mode_validates_auth_and_secret_consistency() {
    let with_password =
        InstanceSecret::for_mode(AuthMode::Password, Some(SecretString::from(CANARY)))
            .expect("password mode with secret");
    assert!(with_password.is_password());

    let no_password =
        InstanceSecret::for_mode(AuthMode::None, None).expect("no-password mode without secret");
    assert!(!no_password.is_password());

    let missing = InstanceSecret::for_mode(AuthMode::Password, None)
        .expect_err("password mode requires a secret");
    assert_eq!(missing.code(), CODE_INVALID_INPUT);
    assert!(!missing.to_string().contains(CANARY));

    let extra = InstanceSecret::for_mode(AuthMode::None, Some(SecretString::from(CANARY)))
        .expect_err("no-password mode must not carry a secret");
    assert_eq!(extra.code(), CODE_INVALID_INPUT);
    assert!(!extra.to_string().contains(CANARY));
}

#[derive(Debug)]
struct FailingRng;

impl rand::TryRng for FailingRng {
    type Error = std::io::Error;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Err(std::io::Error::other("no entropy"))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Err(std::io::Error::other("no entropy"))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        let _ = dst;
        Err(std::io::Error::other("no entropy"))
    }
}

#[test]
fn rng_failure_maps_to_secret_error_without_leaking() {
    let mut rng = FailingRng;
    let err = generate_with(&mut rng, 24).expect_err("rng failure");
    assert_eq!(err.code(), CODE_SECRET);
    let text = format!("{err:?} {} {}", err, err.diagnostic());
    assert!(!text.to_lowercase().contains("entropy"));
    assert!(text.contains(crate::strings::ERR_SECRET));

    // keep the private helper reachable so the mapping stays exercised
    let direct = random_source_failed();
    assert_eq!(direct.code(), CODE_SECRET);
}
