use super::super::terminal::escape_applescript;

/// Inverse of `escape_applescript`: `\\` -> `\`, `\"` -> `"`.
fn unescape_applescript(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let next = chars.next().expect("dangling backslash in escaped text");
            out.push(next);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Every `"` must be preceded by a backslash, and every backslash must be part
/// of a `\\` or `\"` pair. A bare quote or a trailing backslash is a red flag.
fn assert_literal_safe(escaped: &str) {
    let chars: Vec<char> = escaped.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                let next = chars.get(i + 1).copied();
                assert!(
                    matches!(next, Some('\\') | Some('"')),
                    "backslash at {i} not part of an escape pair: {escaped:?}"
                );
                i += 2;
            }
            '"' => panic!("unescaped double quote at {i}: {escaped:?}"),
            _ => i += 1,
        }
    }
}

fn corpus() -> Vec<String> {
    vec![
        "it's".to_owned(),
        "a\"b".to_owned(),
        "a\\b".to_owned(),
        "a\\\\b".to_owned(),
        "line\nbreak".to_owned(),
        "$HOME".to_owned(),
        "x".repeat(4096),
    ]
}

#[test]
fn escape_applescript_round_trips_and_leaves_no_bare_specials() {
    for original in corpus() {
        let escaped = escape_applescript(&original);
        assert_literal_safe(&escaped);
        assert_eq!(
            unescape_applescript(&escaped),
            original,
            "round-trip failed for {original:?}, escaped: {escaped:?}"
        );
    }
}

#[test]
fn escape_applescript_doubles_backslash_and_quotes() {
    assert_eq!(escape_applescript("a\"b"), "a\\\"b");
    assert_eq!(escape_applescript("a\\b"), "a\\\\b");
    assert_eq!(escape_applescript("it's"), "it's");
    assert_eq!(escape_applescript("plain"), "plain");
}
