//! Shell quoting for the rare cases where a terminal requires a command string.
//!
//! The normal path is argv spawning. This module exists only for terminal `-e`
//! forms that accept a single shell command string; the caller must still avoid
//! putting secrets into that string.

/// Quotes a value for a POSIX shell using single quotes.
///
/// Embedded single quotes are escaped as `'\''`, which ends the quoted segment,
/// inserts a literal single quote, and resumes quoting. The result is safe to
/// interpolate into a shell command string as one argument token.
pub fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn quotes_simple_value() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn quotes_empty_value() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn escapes_single_quote() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn preserves_hostile_characters_literally() {
        let hostile = "a b;rm -rf&&$HOME`id`$(id)\n%\t-x";
        let quoted = shell_quote(hostile);
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
        assert!(quoted.contains(";rm -rf&&$HOME`id`$(id)"));
        assert!(quoted.contains('\n'));
    }
}
