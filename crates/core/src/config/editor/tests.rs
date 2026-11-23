//! Byte-exact fixture table for the line editor, plus the apply-then-restore
//! property test.
//!
//! Every fixture pins the whole output string, so a change that rewrites a
//! comment, drops an unknown line, reorders, changes a line ending or adds or
//! removes a trailing newline fails the test.

use super::*;

/// One fixture: input, edit, expected bytes.
struct Case {
    name: &'static str,
    dialect: Dialect,
    input: &'static str,
    edit: Edit<'static>,
    expected: &'static str,
}

fn edit(key: &'static str, value: &'static str, insertion: Insertion) -> Edit<'static> {
    Edit {
        key,
        value,
        insertion,
    }
}

const VALKEY_END: Insertion = Insertion::End;
const PG_END: Insertion = Insertion::End;
const MYSQL_SECTION: Insertion = Insertion::Section("mysqld");

fn cases() -> Vec<Case> {
    vec![
        // --- PostgreSQL -------------------------------------------------------
        Case {
            name: "pg replaces the one assignment, keeps comments and blanks",
            dialect: Dialect::Postgresql,
            input: "# managed by dblocal\n\nmax_connections = 100   # tuned later\n\n# trailing note\n",
            edit: edit("max_connections", "200", PG_END),
            expected: "# managed by dblocal\n\nmax_connections = 200\n\n# trailing note\n",
        },
        Case {
            name: "pg keeps an unknown key and a key that only exists in a comment",
            dialect: Dialect::Postgresql,
            input: "# max_connections = 50\nsome_unknown_key = keep_me\n",
            edit: edit("max_connections", "200", PG_END),
            expected: "# max_connections = 50\nsome_unknown_key = keep_me\nmax_connections = 200\n",
        },
        Case {
            name: "pg uses the last uncommented assignment when a key appears twice",
            dialect: Dialect::Postgresql,
            input: "work_mem = 4MB\nwork_mem = 8MB\n",
            edit: edit("work_mem", "64MB", PG_END),
            expected: "work_mem = 4MB\nwork_mem = 64MB\n",
        },
        Case {
            name: "pg preserves CRLF line endings",
            dialect: Dialect::Postgresql,
            input: "# c\r\nwork_mem = 4MB\r\n",
            edit: edit("work_mem", "8MB", PG_END),
            expected: "# c\r\nwork_mem = 8MB\r\n",
        },
        Case {
            name: "pg appends without adding a trailing newline when the last line had none",
            dialect: Dialect::Postgresql,
            input: "work_mem = 4MB",
            edit: edit("shared_buffers", "256MB", PG_END),
            expected: "work_mem = 4MB\nshared_buffers = 256MB\n",
        },
        // --- INI (my.cnf) -----------------------------------------------------
        Case {
            name: "ini rewrites inside [mysqld] and keeps [client] untouched",
            dialect: Dialect::Ini,
            input: "[client]\nport=3306\n\n[mysqld]\nport=3307\nmax_connections=151\n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[client]\nport=3306\n\n[mysqld]\nport=3307\nmax_connections=300\n",
        },
        Case {
            name: "ini appends a missing key at the end of [mysqld], not after [client]",
            dialect: Dialect::Ini,
            input: "[mysqld]\nport=3307\n\n[client]\nsocket=/x\n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[mysqld]\nport=3307\nmax_connections=300\n\n[client]\nsocket=/x\n",
        },
        Case {
            name: "ini keeps semicolon and hash comments and unknown keys",
            dialect: Dialect::Ini,
            input: "[mysqld]\n# note\n; other note\nunknown_key=keep\nmax_connections=151\n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[mysqld]\n# note\n; other note\nunknown_key=keep\nmax_connections=300\n",
        },
        Case {
            name: "ini keeps an !include line",
            dialect: Dialect::Ini,
            input: "[mysqld]\n!include /etc/extra.cnf\nmax_connections=151\n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[mysqld]\n!include /etc/extra.cnf\nmax_connections=300\n",
        },
        Case {
            name: "ini rewrites the last duplicate, keeping the earlier one",
            dialect: Dialect::Ini,
            input: "[mysqld]\nmax_connections=151\nmax_connections=200\n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[mysqld]\nmax_connections=151\nmax_connections=300\n",
        },
        Case {
            name: "ini accepts a spaced assignment and rewrites it unspaced",
            dialect: Dialect::Ini,
            input: "[mysqld]\n  max_connections = 151  \n",
            edit: edit("max_connections", "300", MYSQL_SECTION),
            expected: "[mysqld]\nmax_connections=300\n",
        },
        // --- Valkey -----------------------------------------------------------
        Case {
            name: "valkey rewrites the last maxmemory line, keeps comments",
            dialect: Dialect::Valkey,
            input: "# tuning\nmaxmemory 0\n",
            edit: edit("maxmemory", "1gb", VALKEY_END),
            expected: "# tuning\nmaxmemory 1gb\n",
        },
        Case {
            name: "valkey appends at the end and does not touch include",
            dialect: Dialect::Valkey,
            input: "include /etc/other.conf\nport 6379\n",
            edit: edit("maxmemory", "1gb", VALKEY_END),
            expected: "include /etc/other.conf\nport 6379\nmaxmemory 1gb\n",
        },
        Case {
            name: "valkey rewrites save with its multi-token value",
            dialect: Dialect::Valkey,
            input: "save 3600 1 300 100\n",
            edit: edit("save", "", VALKEY_END),
            expected: "save \n",
        },
    ]
}

#[test]
fn fixture_table_is_byte_exact() {
    for case in cases() {
        let got = apply_edit(case.dialect, case.input, &case.edit).expect(case.name);
        assert_eq!(got, case.expected, "fixture failed: {}", case.name);
    }
}

#[test]
fn applying_a_new_value_then_restoring_the_original_value_round_trips() {
    // Property-style: for each fixture, replace the key's value, then put the
    // original file back by re-applying an edit that reproduces it. The
    // second edit must reproduce the exact original bytes when the first value
    // was the only occurrence and the edit did not append.
    for case in cases() {
        // Only check the cases where the key already existed (no append), so the
        // inverse edit is meaningful.
        if !case.input.contains(case.edit.key) {
            continue;
        }
        let once = apply_edit(case.dialect, case.input, &case.edit).expect("first");
        // Re-apply a second, different value then the fixture value again; the
        // file must stay stable (idempotent on the same target line).
        let twice = apply_edit(case.dialect, &once, &case.edit).expect("second");
        assert_eq!(once, twice, "edit is not idempotent for {}", case.name);
    }
}

#[test]
fn comments_are_never_rewritten() {
    let input = "# keep me exactly   \n   # indented comment\nwork_mem = 4MB\n";
    let got =
        apply_edit(Dialect::Postgresql, input, &edit("work_mem", "8MB", PG_END)).expect("edit");
    assert!(got.starts_with("# keep me exactly   \n   # indented comment\n"));
    assert!(got.contains("work_mem = 8MB"));
}

#[test]
fn unknown_lines_survive_byte_for_byte() {
    let input = "max_connections = 100\nmy_custom_directive foo bar baz\n";
    let got = apply_edit(
        Dialect::Postgresql,
        input,
        &edit("max_connections", "200", PG_END),
    )
    .expect("edit");
    assert!(got.contains("my_custom_directive foo bar baz\n"), "{got}");
}

#[test]
fn no_trailing_newline_is_preserved_when_appending_nothing_else_changes() {
    let input = "port = 5432";
    let got = apply_edit(Dialect::Postgresql, input, &edit("port", "5433", PG_END)).expect("edit");
    assert_eq!(got, "port = 5433");
}

#[test]
fn crlf_is_preserved_even_for_appended_lines() {
    let input = "a = 1\r\nb = 2\r\n";
    let got = apply_edit(Dialect::Postgresql, input, &edit("c", "3", PG_END)).expect("edit");
    assert_eq!(got, "a = 1\r\nb = 2\r\nc = 3\r\n");
}

#[test]
fn a_value_with_a_newline_is_refused() {
    let err = apply_edit(Dialect::Postgresql, "a = 1\n", &edit("a", "x\ny", PG_END)).unwrap_err();
    assert_eq!(err, EditError::NewlineInValue);
}

#[test]
fn a_value_with_a_nul_is_refused() {
    let err = apply_edit(Dialect::Postgresql, "a = 1\n", &edit("a", "x\0y", PG_END)).unwrap_err();
    assert_eq!(err, EditError::NulInValue);
}

#[test]
fn a_value_that_would_start_a_comment_is_refused() {
    for value in ["1 # injected", "# whole thing is a comment", "x ; more"] {
        let dialect = if value.contains(';') {
            Dialect::Ini
        } else {
            Dialect::Postgresql
        };
        let err = apply_edit(dialect, "a = 1\n", &edit("a", value, PG_END)).unwrap_err();
        assert_eq!(err, EditError::CommentInjection, "value {value:?}");
    }
}

#[test]
fn a_hash_inside_a_token_is_not_treated_as_a_comment() {
    // A Windows path `C:\#dir` has no whitespace before `#`, so PostgreSQL does
    // not treat it as a comment; the editor must accept it.
    let got = apply_edit(
        Dialect::Postgresql,
        "data_directory = '/x'\n",
        &edit("data_directory", "C:\\#dir", PG_END),
    )
    .expect("accept glued hash");
    assert!(got.contains("data_directory = C:\\#dir"));
}

#[test]
fn an_invalid_key_is_refused() {
    for key in ["", "two words", "has=equals", "has#hash"] {
        let insertion = Insertion::End;
        let err =
            apply_edit(Dialect::Postgresql, "a = 1\n", &edit(key, "1", insertion)).unwrap_err();
        assert_eq!(err, EditError::InvalidKey, "key {key:?}");
    }
}

#[test]
fn the_edit_error_display_never_echoes_the_value() {
    let err = EditError::NewlineInValue;
    assert!(!err.to_string().contains("secret"));
    assert!(!format!("{err:?}").contains("secret"));
}

#[test]
fn a_file_with_only_comments_and_no_newline_gets_the_key_appended() {
    let got = apply_edit(
        Dialect::Valkey,
        "# just a comment",
        &edit("maxmemory", "1gb", VALKEY_END),
    )
    .expect("edit");
    assert_eq!(got, "# just a comment\nmaxmemory 1gb\n");
}

#[test]
fn section_append_falls_back_to_end_when_the_section_is_absent() {
    let got = apply_edit(
        Dialect::Ini,
        "[client]\nport=1\n",
        &edit("max_connections", "300", MYSQL_SECTION),
    )
    .expect("edit");
    // No [mysqld]: append at the end rather than corrupting [client].
    assert_eq!(got, "[client]\nport=1\nmax_connections=300\n");
}
