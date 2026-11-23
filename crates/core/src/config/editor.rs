//! Line-level `my.cnf`, `postgresql.conf` and `valkey.conf` editor (PRD
//! `F-CFG-02` AC1).
//!
//! The rule the PRD states is "komentar serta baris yang tidak dikenal app tidak
//! pernah ditulis ulang atau dihapus". A parse-and-re-render editor cannot keep
//! that promise: it reflows the whole file, rewrites comments, drops keys it does
//! not know and normalises line endings. So this editor works on the raw bytes
//! split into lines and changes only the one line of the key it was asked to
//! change. It never reorders, never reflows, and never touches a comment, an
//! unknown key, a blank line, the line ending style or a missing trailing
//! newline.
//!
//! Syntax handled per [`Dialect`]:
//!
//! - `Postgresql`: `key = value`, `key value` and `key` are accepted by the
//!   server; the editor writes and matches `key = value`.
//! - `Ini` (MySQL/MariaDB `my.cnf`): `key=value`, `key = value`, `key = value`
//!   with leading spaces, `#`/`;` comments, `[section]` headers.
//! - `Valkey`: `key value`, `key value1 value2`, `#` comments, `include`/`!include`.
//!
//! "The last uncommented assignment wins, as the engines read it" — so an
//! update rewrites **the last** matching line and leaves earlier duplicates
//! alone (they stay comments/history the user wrote), and an insert appends at
//! the end of the relevant section.

/// Config file syntax family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// `postgresql.conf`: `key = value`, quoted strings, `#` comments.
    Postgresql,
    /// `my.cnf`: `key=value` inside `[section]` blocks, `#`/`;` comments.
    Ini,
    /// `valkey.conf`: `key value...`, `#` comments, `include`.
    Valkey,
}

/// Whether an edit appends inside a section (INI) or at the end of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion {
    /// Append at the very end of the file.
    End,
    /// Append at the end of the named `[section]` (INI dialects).
    Section(&'static str),
}

/// One requested edit: replace the last assignment of `key` with `key<sep>value`,
/// or append one when the key is absent.
#[derive(Debug, Clone)]
pub struct Edit<'a> {
    /// The key to change, exactly as the engine spells it.
    pub key: &'a str,
    /// The new value (already validated; may be an empty string).
    pub value: &'a str,
    /// Where to append the key when it is absent.
    pub insertion: Insertion,
}

/// Why an edit could not be expressed in the file's syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// A value contained `\n` or `\r`, which would create a second line.
    NewlineInValue,
    /// A value contained a NUL byte.
    NulInValue,
    /// A value contained a `#` preceded by whitespace, which would turn the rest
    /// of the line into a comment (the "comment injection" rule).
    CommentInjection,
    /// The key is empty or contains spacing/`=`/`#`, so it cannot be a key.
    InvalidKey,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::NewlineInValue => "value contains a newline",
            Self::NulInValue => "value contains a NUL byte",
            Self::CommentInjection => "value would be read as a comment",
            Self::InvalidKey => "key is not a valid directive name",
        };
        f.write_str(text)
    }
}

/// Edits `text` according to `edit`, returning the new file content.
///
/// The input is treated as the exact bytes to preserve; the output differs only
/// in the one line that changed or in the one line appended. Line endings are
/// preserved: if the file uses CRLF, the appended/edited line uses CRLF and the
/// untouched lines keep their bytes.
pub fn apply_edit(dialect: Dialect, text: &str, edit: &Edit<'_>) -> Result<String, EditError> {
    validate_key(edit.key)?;
    validate_value(dialect, edit.value)?;
    let sep = separator(dialect);
    // Split keeping the exact terminators so untouched lines round-trip.
    let mut lines = split_lines(text);
    let rewritten = format!("{}{sep}{}", edit.key, edit.value);
    let newline = dominant_newline(&lines);

    if let Some(index) = last_assignment(&lines, dialect, edit.key) {
        let line = &mut lines[index];
        line.content = rewritten;
        return Ok(join_lines(&lines));
    }

    // Not present: append.
    let at = match edit.insertion {
        Insertion::End => lines.len(),
        Insertion::Section(name) => section_end(&lines, name).unwrap_or(lines.len()),
    };
    let entry = Line {
        content: rewritten,
        terminator: newline.to_owned(),
    };
    // A file with no trailing newline must gain the newline before our line, and
    // our new line ends with the dominant terminator; the "no trailing newline"
    // property of the previous last line is intentionally preserved only when we
    // did not touch it (we always do when we append).
    if at == lines.len() {
        if let Some(last) = lines.last_mut()
            && last.terminator.is_empty()
        {
            last.terminator = newline.to_owned();
        }
    }
    lines.insert(at, entry);
    Ok(join_lines(&lines))
}

fn validate_key(key: &str) -> Result<(), EditError> {
    if key.is_empty()
        || key
            .chars()
            .any(|c| c.is_whitespace() || c == '=' || c == '#' || c == ';')
    {
        return Err(EditError::InvalidKey);
    }
    Ok(())
}

fn validate_value(dialect: Dialect, value: &str) -> Result<(), EditError> {
    if value.contains('\n') || value.contains('\r') {
        return Err(EditError::NewlineInValue);
    }
    if value.contains('\0') {
        return Err(EditError::NulInValue);
    }
    // A `#` (PostgreSQL/Valkey comment) or `;` (INI comment) that begins a token
    // would turn the value into a comment. A `#` glued to a preceding character
    // (for example inside a Windows path on PostgreSQL) is not a comment to the
    // engine, so only reject a comment character preceded by whitespace or at
    // the start.
    let comment_chars: &[char] = match dialect {
        Dialect::Ini => &['#', ';'],
        Dialect::Postgresql | Dialect::Valkey => &['#'],
    };
    let mut previous: Option<char> = None;
    for ch in value.chars() {
        if comment_chars.contains(&ch) && previous.is_none_or(char::is_whitespace) {
            return Err(EditError::CommentInjection);
        }
        previous = Some(ch);
    }
    Ok(())
}

fn separator(dialect: Dialect) -> &'static str {
    match dialect {
        // PostgreSQL's own files and docs spell it `key = value`.
        Dialect::Postgresql => " = ",
        Dialect::Ini => "=",
        Dialect::Valkey => " ",
    }
}

/// A physical line: its text without the terminator, plus the terminator.
#[derive(Debug, Clone)]
struct Line {
    content: String,
    terminator: String,
}

fn split_lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(idx) => {
                let (head, tail) = rest.split_at(idx);
                let (content, term) = if let Some(stripped) = head.strip_suffix('\r') {
                    (stripped.to_owned(), "\r\n".to_owned())
                } else {
                    (head.to_owned(), "\n".to_owned())
                };
                out.push(Line {
                    content,
                    terminator: term,
                });
                rest = &tail[1..];
            }
            None => {
                out.push(Line {
                    content: rest.to_owned(),
                    terminator: String::new(),
                });
                rest = "";
            }
        }
    }
    out
}

fn dominant_newline(lines: &[Line]) -> &'static str {
    if lines.iter().any(|l| l.terminator == "\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn join_lines(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.content);
        out.push_str(&line.terminator);
    }
    out
}

/// Returns the index of the last uncommented assignment of `key`.
fn last_assignment(lines: &[Line], dialect: Dialect, key: &str) -> Option<usize> {
    let mut found = None;
    for (index, line) in lines.iter().enumerate() {
        if assignment_key(dialect, &line.content).as_deref() == Some(key) {
            found = Some(index);
        }
    }
    found
}

/// Returns the key of a line that is an uncommented assignment, if it is one.
///
/// Comment lines (`#`, `;`), section headers (`[mysqld]`) and blank lines are
/// not assignments. For Valkey, an `include`/`!include` line is not an
/// assignment to the included file's first token.
fn assignment_key(dialect: Dialect, line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with(';')
        || trimmed.starts_with('[')
    {
        return None;
    }
    match dialect {
        Dialect::Postgresql => {
            // `key = value`, `key value`, `key`.
            let rest = trimmed;
            let key = split_key(rest)?;
            Some(key.to_owned())
        }
        Dialect::Ini => {
            // `key=value` or `key = value`; a line with no `=` is not one.
            let (key, _value) = trimmed.split_once('=')?;
            Some(key.trim().to_owned())
        }
        Dialect::Valkey => {
            // `key value...` or `key`.
            let key = trimmed.split_whitespace().next()?;
            if key.eq_ignore_ascii_case("include") {
                return None;
            }
            Some(key.to_owned())
        }
    }
}

/// Splits a PostgreSQL directive into its key: the leading run of characters
/// that are not whitespace, `=` or `#`.
fn split_key(line: &str) -> Option<&str> {
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=' || c == '#')
        .unwrap_or(line.len());
    let key = &line[..end];
    if key.is_empty() { None } else { Some(key) }
}

/// Index just past the last non-blank line that belongs to `[name]`.
///
/// A section's trailing blank lines stay where they are (they usually separate
/// it from the next section), so an appended key lands directly under the last
/// setting, not after the separator.
fn section_end(lines: &[Line], name: &str) -> Option<usize> {
    let header = format!("[{name}]");
    let start = lines
        .iter()
        .position(|l| l.content.trim().eq_ignore_ascii_case(&header))?;
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.content.trim_start().starts_with('[') {
            end = index;
            break;
        }
    }
    while end > start + 1 && lines[end - 1].content.trim().is_empty() {
        end -= 1;
    }
    Some(end)
}

#[cfg(test)]
mod tests;
