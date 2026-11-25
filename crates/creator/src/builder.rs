use std::{collections::HashMap, path::PathBuf};

use nimbus_vault::{config::OriginConfig, origin::http::HttpAuth};

/// A single field the wizard needs to collect for the currently selected [`OriginKind`].
#[derive(Clone, Copy)]
pub struct FieldSpec {
    /// Key `values` is stored/looked up under; matches the field name on the
    /// corresponding `OriginConfig` variant.
    pub key: &'static str,
    /// Prompt shown to the user.
    pub label: &'static str,
    /// Whether an empty input is accepted (maps to `None`/omitted on the built config).
    pub optional: bool,
    /// Whether `Tab` should offer filesystem-path completion while editing this field.
    pub path_completable: bool,
}

const fn field(key: &'static str, label: &'static str, optional: bool) -> FieldSpec {
    FieldSpec {
        key,
        label,
        optional,
        path_completable: false,
    }
}

const fn path_field(key: &'static str, label: &'static str, optional: bool) -> FieldSpec {
    FieldSpec {
        key,
        label,
        optional,
        path_completable: true,
    }
}

/// The origin types a vault can be built with, mirroring
/// [`nimbus_vault::config::OriginConfig`]'s variants.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OriginKind {
    Fs,
    Http,
    Command,
    Vault,
}

impl OriginKind {
    pub const ALL: [OriginKind; 4] = [
        OriginKind::Fs,
        OriginKind::Http,
        OriginKind::Command,
        OriginKind::Vault,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            OriginKind::Fs => "fs — a directory on disk",
            OriginKind::Http => "http — a REST-ish HTTP API",
            OriginKind::Command => "command — a shell command per operation",
            OriginKind::Vault => "vault — another vault's config file",
        }
    }

    /// The fields the wizard must collect for this origin kind, in prompt order.
    pub fn fields(&self) -> Vec<FieldSpec> {
        match self {
            OriginKind::Fs => vec![path_field(
                "root",
                "root directory (tab to complete)",
                false,
            )],
            OriginKind::Http => vec![
                field("base_url", "base url (optional)", true),
                field("list_url", "list url ({id}-templated)", false),
                field("fetch_url", "fetch url ({id}-templated)", false),
                field("get_url", "get url ({id}-templated)", false),
                field("put_url", "put url ({id}-templated)", false),
                field("send_url", "send url ({id}-templated)", false),
                field("delete_url", "delete url ({id}-templated)", false),
                // The wizard only offers the env-var form, which is the one that keeps the
                // secret out of a config file people copy around. `token`/`token_file` are
                // there for hand-edited configs — see `HttpAuth`.
                field(
                    "auth_token_env",
                    "bearer token env var, e.g. NIMBUS_TOKEN (optional)",
                    true,
                ),
            ],
            OriginKind::Command => vec![
                field("list_cmd", "list command", false),
                field("fetch_cmd", "fetch command", false),
                field("get_cmd", "get command", false),
                field("put_cmd", "put command", false),
                field("send_cmd", "send command", false),
                field("delete_cmd", "delete command", false),
                field("extras", "extra vars, k=v,k2=v2 (optional)", true),
            ],
            OriginKind::Vault => vec![field("path", "inner vault config path", false)],
        }
    }

    fn parse_extras(raw: &str) -> HashMap<String, String> {
        raw.split(',')
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                let k = k.trim();
                let v = v.trim();
                if k.is_empty() {
                    None
                } else {
                    Some((k.to_string(), v.to_string()))
                }
            })
            .collect()
    }

    /// Builds the [`OriginConfig`] this kind describes, out of the field values collected by
    /// the wizard (keyed by [`FieldSpec::key`]).
    pub fn build(&self, values: &HashMap<String, String>) -> OriginConfig {
        let get = |key: &str| values.get(key).cloned().unwrap_or_default();
        match self {
            OriginKind::Fs => OriginConfig::Fs {
                root: absolute_answer(get("root")),
            },
            OriginKind::Http => {
                let base_url = get("base_url");
                let token_env = get("auth_token_env");
                OriginConfig::Http {
                    base_url: if base_url.is_empty() {
                        None
                    } else {
                        Some(base_url)
                    },
                    list_url: get("list_url"),
                    fetch_url: get("fetch_url"),
                    get_url: get("get_url"),
                    put_url: get("put_url"),
                    send_url: get("send_url"),
                    delete_url: get("delete_url"),
                    rename_url: None,
                    tls: None,
                    auth: if token_env.is_empty() {
                        HttpAuth::None
                    } else {
                        HttpAuth::Bearer {
                            token: None,
                            token_env: Some(token_env),
                            token_file: None,
                        }
                    },
                }
            }
            OriginKind::Command => {
                let extras_raw = get("extras");
                let extras = if extras_raw.is_empty() {
                    None
                } else {
                    Some(Self::parse_extras(&extras_raw))
                };
                OriginConfig::Command {
                    list_cmd: get("list_cmd"),
                    fetch_cmd: get("fetch_cmd"),
                    get_cmd: get("get_cmd"),
                    put_cmd: get("put_cmd"),
                    send_cmd: get("send_cmd"),
                    delete_cmd: get("delete_cmd"),
                    extras,
                    rename_cmd: None,
                }
            }
            OriginKind::Vault => OriginConfig::Vault {
                path: absolute_answer(get("path")),
            },
        }
    }
}

/// A path typed at a wizard prompt, made absolute the way a shell would read it: a leading `~`
/// is the home directory ([`expand_tilde`]), and anything else relative is relative to the
/// current directory, which is what the prompt completes against. A saved config resolves its
/// relative paths against its own directory instead, so the answer is stored absolute to keep
/// the meaning the user typed, whether or not they pressed Tab. An answer that can't be made
/// absolute (an empty one, or an unreadable cwd) is kept as typed.
fn absolute_answer(answer: String) -> PathBuf {
    let path = PathBuf::from(expand_tilde(&answer));
    std::path::absolute(&path).unwrap_or(path)
}

/// Expands a leading `~` or `~/...` to the user's home directory, so typed paths behave like
/// they would in a shell. Left untouched if there's no such prefix (`~user` included, which
/// names another user's home) or the home dir is unknown.
pub(crate) fn expand_tilde(input: &str) -> String {
    let rest = match input.strip_prefix('~') {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => return input.to_string(),
    };
    let Some(home) = dirs::home_dir() else {
        return input.to_string();
    };
    if rest.is_empty() {
        home.to_string_lossy().into_owned()
    } else {
        home.join(rest).to_string_lossy().into_owned()
    }
}
