//! [`parse_toml`](crate::parse::parse_toml): reads a config file's TOML into a type, with
//! errors that never quote the file. A config can hold a secret (an inline bearer token), and
//! both the TOML parser and serde echo what they choke on: the parser prints the offending
//! source line, and serde's messages quote the offending value (`invalid type: integer
//! 12345678`). Such an error ends up on a terminal, in daemon logs, in bug reports. So a parse
//! error here says only *where* (file, line, column) and *what kind*, never the text it found.
//! The only name an error ever carries is a missing field's, which comes from the schema, never
//! from the file: an unknown key is user text (a secret pasted on its own line becomes a key).
//!
//! # Examples
//!
//! ```
//! use std::path::Path;
//! use nimbus_vault::parse::{parse_toml, ParseErrorKind};
//!
//! #[derive(serde::Deserialize, Debug)]
//! struct Auth {
//!     token: String,
//! }
//!
//! let error = parse_toml::<Auth>("token = 12345678\n", Path::new("vault.toml")).unwrap_err();
//! assert_eq!(error.kind, ParseErrorKind::WrongType);
//! assert_eq!(error.to_string(), "vault.toml: wrong type at line 1, column 9");
//! assert!(!format!("{error:?}").contains("12345678"));
//! ```

use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

/// Parses `raw`, the contents of the config file at `path`, as TOML into a `T`. On failure the
/// error names `path` and the position, but carries none of the file's text; see the
/// [module docs](crate::parse).
pub fn parse_toml<T: DeserializeOwned>(raw: &str, path: &Path) -> Result<T, ConfigParseError> {
    toml::from_str(raw).map_err(|error| ConfigParseError::new(path, raw, &error))
}

/// A config file that didn't parse, described without quoting any of it.
///
/// `line` and `column` are where the parser points, which isn't always the offending value: for
/// a mistake inside a sub-table, they point at the header of the enclosing table serde reads as
/// a whole (for a vault config, `[origin_config]`, since serde buffers that internally tagged
/// table before looking inside it). That is toml's span, not a bug here. There is deliberately no key name: the
/// only way to find one would be to read it from the source, and a line inside a multi-line
/// string looks just like `key = …` or `[table]`, so the "key" could be part of a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {kind}{}", path.display(), self.location())]
pub struct ConfigParseError {
    /// The file that failed to parse.
    pub path: PathBuf,
    /// What went wrong, in general terms.
    pub kind: ParseErrorKind,
    /// 1-based line of the problem, when the parser reported a position.
    pub line: Option<usize>,
    /// 1-based column (in characters) of the problem, when the parser reported a position.
    pub column: Option<usize>,
}

/// What kind of mistake a [`ConfigParseError`] is. Deliberately coarse: the finer detail
/// serde and the TOML parser give quotes the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Not valid TOML at all.
    Syntax,
    /// A value of the wrong type for its key (a number where a string goes, say).
    WrongType,
    /// A value of the right type that isn't one of the accepted ones.
    InvalidValue,
    /// A `type = …` naming a variant that doesn't exist.
    UnknownVariant,
    /// A list or tuple of the wrong length.
    WrongLength,
    /// A key the config doesn't know. Not named: the key is the user's text, and a secret
    /// written on a line of its own (`SECRET = 1`) parses as one.
    UnknownField,
    /// A required key that is absent. The name comes from the schema.
    MissingField(String),
    /// The same key set twice.
    DuplicateKey,
}

impl std::fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseErrorKind::Syntax => f.write_str("syntax error"),
            ParseErrorKind::WrongType => f.write_str("wrong type"),
            ParseErrorKind::InvalidValue => f.write_str("invalid value"),
            ParseErrorKind::UnknownVariant => f.write_str("unknown variant"),
            ParseErrorKind::WrongLength => f.write_str("wrong number of elements"),
            ParseErrorKind::UnknownField => f.write_str("unknown field"),
            ParseErrorKind::MissingField(name) => write!(f, "missing field `{name}`"),
            ParseErrorKind::DuplicateKey => f.write_str("duplicate key"),
        }
    }
}

impl ConfigParseError {
    /// Describes `error` from parsing `raw` (the file at `path`). Reads only the error's span
    /// and the start of its message, which says which serde check failed; the rest of the
    /// message, and `error`'s own `Display`, are never kept.
    fn new(path: &Path, raw: &str, error: &toml::de::Error) -> Self {
        let position = error.span().map(|span| position_of(raw, span.start));
        ConfigParseError {
            path: path.to_path_buf(),
            kind: classify(error.message()),
            line: position.map(|(line, _)| line),
            column: position.map(|(_, column)| column),
        }
    }

    /// `" at line L, column C"`, or nothing when the parser reported no position.
    fn location(&self) -> String {
        match (self.line, self.column) {
            (Some(line), Some(column)) => format!(" at line {line}, column {column}"),
            _ => String::new(),
        }
    }
}

/// Maps serde's message to a [`ParseErrorKind`] by its fixed prefix. Only a missing field's
/// name, which serde puts in the first pair of backticks and takes from the schema, is ever
/// copied out; a message that matches no serde check is the TOML parser's, so a syntax error.
fn classify(message: &str) -> ParseErrorKind {
    let field = || first_backticked(message).unwrap_or_default();
    if message.starts_with("invalid type") {
        ParseErrorKind::WrongType
    } else if message.starts_with("invalid value") {
        ParseErrorKind::InvalidValue
    } else if message.starts_with("unknown variant") {
        ParseErrorKind::UnknownVariant
    } else if message.starts_with("invalid length") {
        ParseErrorKind::WrongLength
    } else if message.starts_with("unknown field") {
        ParseErrorKind::UnknownField
    } else if message.starts_with("missing field") {
        ParseErrorKind::MissingField(field())
    } else if message.starts_with("duplicate") {
        ParseErrorKind::DuplicateKey
    } else {
        ParseErrorKind::Syntax
    }
}

/// The text between the first two backticks of `message`, if it looks like a key name.
fn first_backticked(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let len = message[start..].find('`')?;
    let name = &message[start..start + len];
    is_bare_key(name).then(|| name.to_string())
}

/// 1-based (line, column) of byte `offset` in `raw`, the column counted in characters.
fn position_of(raw: &str, offset: usize) -> (usize, usize) {
    let end = (0..=offset.min(raw.len()))
        .rev()
        .find(|&i| raw.is_char_boundary(i))
        .unwrap_or(0);
    let before = &raw[..end];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// Whether `name` is a TOML bare key: non-empty, only ASCII letters, digits, `_` and `-`.
fn is_bare_key(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
#[path = "tests/parse.rs"]
mod tests;
