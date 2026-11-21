//! Masking of the connection URI and the `.env` block.
//!
//! Passwords are `SecretString`s and must never reach a `Label`, clipboard,
//! toast or log unless the user asks. The view builds every hidden string
//! through [`mask_uri`] / [`mask_dotenv_with_secret`], which keep the shape
//! readable while replacing the secret in every form it can take (raw,
//! percent-encoded and URI userinfo).

/// The mask token written over a hidden secret.
pub const SECRET_MASK: &str = "••••••";

/// Masks the password inside a connection URI, keeping the shape readable.
///
/// `postgres://postgres:hunter2@127.0.0.1:5432/db` becomes
/// `postgres://postgres:••••••@127.0.0.1:5432/db`.
pub fn mask_uri(uri: &str) -> String {
    let Some((scheme, _)) = uri.split_once("://") else {
        return uri.to_owned();
    };
    let rest = &uri[scheme.len() + 3..];
    let Some(at) = rest.rfind('@') else {
        return uri.to_owned();
    };
    let credentials = &rest[..at];
    let Some((user, _password)) = credentials.split_once(':') else {
        // Redis without a user has no colon before the `@`; leave as-is.
        return uri.to_owned();
    };
    format!("{scheme}://{user}:{SECRET_MASK}{}", &rest[at..])
}

/// Masks only the password value in a `.env` block.
///
/// This is the key-based path: it covers `*PASSWORD` keys and any value with
/// URI userinfo, but not a URI whose userinfo is unknown to it. Prefer
/// [`mask_dotenv_with_secret`] in the view, which also knows the secret.
pub fn mask_dotenv(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for (index, line) in block.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&mask_env_line(line, None));
    }
    if block.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Masks a `.env` block, replacing the known secret everywhere.
///
/// The known secret is taken from the connection URI the view already holds,
/// so a `DATABASE_URL`/`REDIS_URL` line that embeds userinfo is masked even
/// though its key does not end in `PASSWORD`. Any value with
/// `scheme://user:pass@` userinfo and any `*PASSWORD` key is masked too, so an
/// unknown shape cannot leak either. Both the raw and the percent-encoded
/// forms of the secret are replaced.
pub fn mask_dotenv_with_secret(block: &str, uri: &str) -> String {
    let secret = uri_password(uri);
    let encoded = secret.as_deref().map(encode_userinfo);
    let mut out = String::with_capacity(block.len());
    for (index, line) in block.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let line = match secret.as_deref() {
            Some(secret) => mask_occurrences(line, secret),
            None => line.to_owned(),
        };
        let line = match encoded.as_deref() {
            Some(encoded) => mask_occurrences(&line, encoded),
            None => line,
        };
        out.push_str(&mask_env_line(&line, secret.as_deref()));
    }
    if block.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Percent-encodes a URI userinfo value exactly as the core builder does.
fn encode_userinfo(value: &str) -> String {
    dblocal_core::engine::uri::encode_userinfo(value)
}

/// Percent-decodes a URI userinfo value. Invalid sequences are kept as-is.
fn decode_userinfo(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The password from `scheme://user:pass@host`; `None` when there is no userinfo.
///
/// Returns the raw (percent-decoded) password, because the `.env` block stores
/// the raw password while the URI stores the encoded form.
fn uri_password(uri: &str) -> Option<String> {
    let (_, rest) = uri.split_once("://")?;
    let at = rest.rfind('@')?;
    let credentials = &rest[..at];
    let (_, password) = credentials.split_once(':')?;
    if password.is_empty() {
        None
    } else {
        Some(decode_userinfo(password))
    }
}

/// Replaces every occurrence of `needle` with the mask, skipping empty needles.
fn mask_occurrences(text: &str, needle: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    text.replace(needle, SECRET_MASK)
}

/// Masks one `.env` line: password-like keys, plus any value with URI userinfo.
fn mask_env_line(line: &str, known_secret: Option<&str>) -> String {
    let Some((key, value)) = line.split_once('=') else {
        return line.to_owned();
    };
    if value.is_empty() {
        return line.to_owned();
    }
    let has_userinfo = value.contains("://") && value.contains('@');
    if has_userinfo {
        return format!("{key}={}", mask_uri_with_secret(value, known_secret));
    }
    if is_password_key(key) {
        return format!("{key}={SECRET_MASK}");
    }
    line.to_owned()
}

/// Keys whose whole value is a secret.
fn is_password_key(key: &str) -> bool {
    key.to_ascii_uppercase().ends_with("PASSWORD")
}

/// Masks a URI, replacing any known secret as well as the userinfo password.
fn mask_uri_with_secret(uri: &str, known_secret: Option<&str>) -> String {
    let mut text = match known_secret {
        Some(secret) if !secret.is_empty() => mask_occurrences(uri, secret),
        _ => uri.to_owned(),
    };
    if let Some(encoded) = known_secret.map(encode_userinfo)
        && !encoded.is_empty()
    {
        text = mask_occurrences(&text, &encoded);
    }
    mask_uri(&text)
}
