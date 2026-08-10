# nimbus-vault

Library crate for the Nimbus CLI/TUI. Abstracts a remote or local "origin" — a filesystem, a shell command, an HTTP API — behind a single tree-like `Vault` structure, similar to a filesystem, regardless of where the underlying objects actually live.

```bash
cargo add nimbus-vault
```

It's usable on its own: nothing here depends on the frontends, and a `Vault` can be opened from a `vault.toml` (or an `Origin` built without a vault at all, see [Building an origin without a vault](#building-an-origin-without-a-vault)) in a few lines.

[`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui) (`cargo install nimbus-tui`) is a file manager built on this crate, and the fastest way to watch every origin type below behave identically from the outside. Between the two sits [`nimbus-core`](https://github.com/PeachGB/nimbus/tree/main/crates/core), the shared core of the frontends: it turns what's here into an application, with a vault registry, a current directory and sessions on top.

## What's here

- **`vault.rs`** — `Vault`: holds a name, an `Arc<dyn Origin>`, a local `Object` cache (`Mutex<HashMap<ObjectId, Object>>`), and a single root `ObjectId`. `Vault::new(path)` reads a `VaultConfig` from a TOML file at `path` and bootstraps the origin and root id from it. `find(path)` walks a filesystem-style `PathBuf` component by component, calling `list` at each level and matching child names, to resolve it to an `ObjectId` starting from the root. `get` serves from the cache when present, falling back to the origin and caching the result; `list` always hits the origin but caches every child it returns (and updates the parent's cached child-id list, if the parent is itself cached as a `Branch`/`Root`); `delete` evicts an id from the cache. `fetch`/`send` stream payloads straight through the origin and are never cached. `put(object, destination)` writes `object` under `destination` and caches (and returns) the `Object` the origin reports back — not the input — since an origin isn't required to rename `object` in place (see [The `put` contract](#the-put-contract)); on failure, nothing is cached. `pull(id, remote)`/`push(id, remote)` recursively sync `id`'s subtree between this vault's origin and an arbitrary `&dyn Origin`, threading `put`'s returned `Object` through to the following `send` — see [Syncing between origins](#syncing-between-origins) below.
- **`config.rs`** — `VaultConfig`: the on-disk shape of a vault (`name`, an optional `root_id` defaulting to `"/"`, plus an `origin_config`), and `OriginConfig`, a tagged enum (`type = "fs" | "command" | "http" | "vault"`) describing how to build the origin declaratively — the `http` variant also carries an optional `auth` table (`HttpAuth`), which is why it's the variant's last field: it's the only one that serializes as a TOML table, and a table has to come after every plain value in the same struct or `VaultConfig::save` fails on a config it just built. `VaultConfig::build(path)` reads and parses the TOML file and returns the vault's name, root id, and the matching `Box<dyn Origin>`. `OriginConfig::from_file(path)` builds just the `Box<dyn Origin>` from a TOML file containing only the `origin_config` shape (no `name`/`root_id`), for callers that want to talk to an origin directly without opening a `Vault` — see [Building an origin without a vault](#building-an-origin-without-a-vault). `OriginConfig::build(self)` takes `self` by value rather than `&self`, so it moves each variant's fields straight into the `Origin` it constructs instead of cloning them. `VaultConfig::save(path)` writes the config out, creating the parent directory if needed; `VaultConfig::default_dir()`/`default_path(name)` give the conventional location for a new vault's config (see [Where configs live](#where-configs-live)).
- **`object.rs`** — `Object` (`Leaf` / `Branch` / `Root` variants), `ObjectId` (newtype around `String`, defaults to `ROOT_ID` (`"/"`), with `is_root()`), `Metadata` (size, content type, modified time, free-form `extra` map, plus `hash_value()` for a stable content hash). `Object::push` appends a child id onto a `Branch`/`Root`; `Object::with_id` overwrites an object's id in place (used by origins that rename an object on `put`, e.g. `OriginFileSystem`); `Object::with_name` does the same for the name, which is what turns a copy into a rename — `Origin::put` writes an object under its own `get_name()`, so setting the name before `put` is the only rename primitive there is (both are no-ops for `Root`). `Object::get_name()` returns `ROOT_NAME` (`"##ROOT##"`) for `Root`, which has no real name. `Object::changed(remote)` compares `hash_value()` on both sides' metadata to detect drift between a local and remote copy of the same object, returning `false` if either side has no metadata (e.g. `Root`).
- **`origin/mod.rs`** — the `Origin` trait (`fetch`, `list`, `get`, `put`, `send`, `delete`, plus `Send + Sync`) that every backend implements.
- **`origin/fs.rs`** — `OriginFileSystem`: origin backed by a directory on disk. `ObjectId`s are relative paths under `root`. `put` resolves the object's path as `{destination}/{name}`, renames the object in place (via `Object::with_id`) to that path, and creates the file/directory (truncating, for a `Leaf` — `put` creates, `send` fills in). `send` explicitly `flush`es before returning: `tokio::fs::File` buffers, and its `Drop` only starts a best-effort background flush, so without it the bytes weren't guaranteed on disk when `send` returned and a read straight afterwards could come back short or empty.
- **`origin/command.rs`** — `OriginCommand`: origin backed by shell commands, one per operation (`fetch_cmd`, `list_cmd`, `get_cmd`, `put_cmd`, `send_cmd`, `delete_cmd`). Every command runs under `sh -c` and is `{placeholder}`-templated with the object id, name, metadata and kind, plus arbitrary `extra_vars` (guarded by an internal `futures::lock::Mutex`, since `put` needs to set a `destination` var without requiring `&mut self`); `list`/`get` expect the command's stdout to be JSON matching `Object`. `put` runs `put_cmd`, then re-`get`s `"{destination}/{name}"` to return the stored `Object` — it does **not** rename the input object in place, unlike `OriginFileSystem`. See [Command templating](#command-templating) for which placeholders reach which template.
- **`origin/http.rs`** — `OriginHTTP`: origin backed by a REST-ish HTTP API. `base_url` plus a `{id}`-templated path per operation (`fetch_url`, `list_url`, `get_url`, `put_url`, `send_url`, `delete_url`). Every URL is `base_url` (trailing `/` trimmed) followed by that operation's template with `{id}` substituted. `get`/`list` are `GET`s deserialized as JSON; `fetch` streams the response body; `put` `PUT`s the `Object` as JSON to `put_url` — with `{id}` filled in from the **destination**, not the object — then re-`get`s `"{destination}/{name}"` to return the stored `Object` (again, without mutating the input); `send` `PUT`s the payload stream as the request body of `send_url` (templated with the object's own id); `delete` is a `DELETE`. Any non-2xx response becomes a `VaultError`, with 404 mapped to `NotFound` and 401/403 to an error naming `[origin_config.auth]`, since that's where the cause is. Credentials come from `HttpAuth` (`with_auth`, or an `[auth]` table inside `[origin_config]`) and are attached in the one place every request passes through, so an operation added later can't forget to authenticate — see [Authenticating an HTTP origin](#authenticating-an-http-origin).
- **`origin/vault.rs`** — `OriginVault`: origin backed by another `Vault` (held as `Arc<Vault>`). Every trait method just forwards to the wrapped vault's method of the same name — see [Using a vault as an origin](#using-a-vault-as-an-origin).
- **`error.rs`** — `VaultError` (`thiserror`-based) / `VaultResult<T>`, the error type used across the crate. `NotFound` carries the id/url/name that wasn't found; `Io`, `Json`, `Toml`, and `HTTP` wrap the corresponding std/serde/toml/reqwest errors via `#[from]`.

## Origin trait

```rust
#[async_trait::async_trait]
pub trait Origin: Send + Sync {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream>;
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>>;
    async fn get(&self, id: &ObjectId) -> VaultResult<Object>;
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object>;
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
    async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
}
```

`ByteStream` is a boxed, pinned stream of `VaultResult<Bytes>` — used for both reading (`fetch`) and writing (`send`) object payloads without buffering the whole thing in memory.

## The `put` contract

`put(object, destination)` writes `object` under the directory-like `destination`, and returns the `Object` as it now exists at the origin. Callers should always use **the returned `Object`**, not `object` itself, for anything downstream (caching, choosing where to `send` a leaf's payload, etc.) — an origin is allowed, but not required, to rename `object` in place via `Object::with_id`:

- `OriginFileSystem` renames `object` in place to `"{destination}/{name}"` (so `object.get_id()` is accurate after the call) *and* returns a matching clone.
- `OriginHTTP`/`OriginCommand` leave the input `object` untouched and instead compute `"{destination}/{name}"`, re-fetch it via `get`, and return that.

`Vault::put` and `Vault::pull`/`Vault::push` follow this contract: they cache and act on `put`'s return value, and only cache on success. Passing the stale input object to a later `send`/cache-insert (instead of the value `put` returned) is the most likely bug to reintroduce if this file changes again — the `RenamingOrigin` test mock in `vault.rs` (used by `put_caches_under_the_returned_id_not_the_input_id`, `put_does_not_cache_when_origin_put_fails`, and the `pull`/`push` "sends to the id put actually returned" tests) exists specifically to catch it, by mimicking `OriginHTTP`/`OriginCommand`'s no-mutation behavior.

## Syncing between origins

`Vault::pull(id, remote)` and `Vault::push(id, remote)` recursively sync the subtree rooted at `id` between this vault's own origin and any other `&dyn Origin`:

```rust
// bring the vault's local origin up to date with `remote`, starting at the vault's root
vault.pull(&root_id, remote.as_ref()).await?;

// push the vault's local subtree out to `remote`
vault.push(&root_id, remote.as_ref()).await?;
```

Both walk one `list` level at a time (`remote.list`/`self.list`, respectively) and, for every child:

1. Look up the corresponding object on the other side (`self.get`/`remote.get`). A `NotFound` means the object doesn't exist there yet; any other error aborts the whole sync.
2. If the object is missing, or present but `Object::changed` reports the metadata hashes diverge, `put` the object on the destination and — for `Leaf`s only — `fetch` the payload from the source and `send` it to **the `Object` `put` returned** (not the pre-`put` local variable — see [The `put` contract](#the-put-contract)).
3. If the child is a `Branch`/`Root`, recurse into it regardless of whether it itself needed syncing, so descendants are still visited.

`pull` and `push` are mirror images of each other (`pull` reads from `remote`/writes to `self`; `push` reads from `self`/writes to `remote`), so the same object is never treated as changed just because a `Root`/`Branch` container's own (nonexistent) metadata differs — see `Object::changed`.

## Command templating

`OriginCommand` substitutes `{placeholder}`s into each template before running it under `sh -c`.
Which placeholders are available depends on the operation, because `put`/`send` are handed an
`Object` while everything else is handed only an id:

| Template | Gets |
|----------|------|
| `list_cmd`, `fetch_cmd`, `get_cmd`, `delete_cmd` | `{id}` + configured `extras` |
| `put_cmd`, `send_cmd` | `{id}`, `{name}`, `{size}`, `{content_type}`, `{modified}`, `{kind}`, `{destination}`, the object's own `meta.extra`, + configured `extras` |

`{kind}` is `leaf` or `branch`. Without it a `put_cmd` has no way to tell whether it is being
asked to create a file or a directory, which makes `mkdir` impossible to implement against a
command origin.

`{destination}` is the parent id `put` was called with, and is **refreshed on every call**.
`extra_vars` outlives the operation, so recording it once and reusing it would silently send every
later `put` to wherever the first one happened to go. That it reaches `send_cmd` at all is a
side effect of the same lifetime — `send` never sets it, so it holds whatever the last `put` left
behind, and a `send_cmd` run before any `put` finds the literal `{destination}` still in the
string. `send_cmd` templates are better written against `{id}`, which is always the object's own.

The object's own `meta.extra` is applied before the configured `extras`, so a per-object value
beats a config default.

## Where configs live

`config_home()` returns `.nimbus` under the platform config directory (`$XDG_CONFIG_HOME` on
Linux), falling back to the temp directory. It's defined here rather than in each binary so the
CLI's own config and the vault configs written by the creator wizard can't drift apart —
[`nimbus-core`](https://github.com/PeachGB/nimbus/tree/main/crates/core)'s `CliConfig::path()` calls it too.

- `VaultConfig::default_dir()` — `<config_home>/vaults`, where new vault configs go.
- `VaultConfig::default_path(name)` — `<config_home>/vaults/<name>.toml`.

`VaultConfig::save` creates the parent directory as needed, since that directory doesn't exist
until the first vault is created. It writes through, so **callers are responsible for refusing to
overwrite** anything they didn't mean to — the creator wizard checks before saving, because its
default path is derived from the vault name and reusing a name would otherwise destroy the
existing config.

## Building an origin without a vault

`OriginConfig::from_file(path)` reads just the `origin_config` shape from a TOML file and constructs the matching `Box<dyn Origin>` — no `name`, `root_id`, or `Vault` required:

```toml
# origin.toml
type = "fs"
root = "/srv/data"
```

```rust
let origin: Box<dyn Origin> = OriginConfig::from_file("origin.toml".into())?;
```

This is useful for tooling that talks to an origin directly (e.g. syncing two origins with `push`/`pull` without needing a `Vault` on either side) or for building an `Origin` to pass as the `remote` argument to `Vault::pull`/`Vault::push`.

## Authenticating an HTTP origin

An `http` origin sends no credentials unless it's given some. `HttpAuth` is a tagged enum, so a
new scheme is an added variant rather than a breaking change to existing configs; today it's
`none` (the default) or `bearer`:

```toml
[origin_config]
type = "http"
base_url = "http://server:8080/v/photos"
# … the six url templates …

[origin_config.auth]
type = "bearer"
token_env = "NIMBUS_TOKEN"
```

The secret comes from exactly one of `token` (written in the config), `token_env` (an
environment variable) or `token_file` (a file, whose trailing newline isn't part of the token).
Two at once is an error rather than a precedence rule — silently preferring one would let a
stale `token` override the `token_env` someone added to replace it.

Reach for `token_env`. A vault config is a file people copy between machines and commit, which
is no place for a password; `~/.config/.nimbus/vaults/` is not a secret store. The wizard in
[`nimbus-creator`](https://github.com/PeachGB/nimbus/tree/main/crates/creator) only offers that form for the same reason.

The secret is resolved when the origin is built, not per request, so a missing variable or an
unreadable file surfaces as the vault is opened rather than as a 401 halfway through a sync.
A vault that fails to open this way stays *registered* — an unset variable in one shell doesn't
unregister it (see [`nimbus-core`](https://github.com/PeachGB/nimbus/tree/main/crates/core)).

There is no TLS here beyond whatever `base_url` names: a bearer token on plaintext `http://` is
readable by anything on the path.

## Using a vault as an origin

`OriginVault` wraps an `Arc<Vault>` and implements `Origin` by forwarding every call to the wrapped vault's own method of the same name. This lets one `Vault` act as the `remote` for another vault's `push`/`pull`, so two vaults can sync with each other directly without either one needing to know the other is a `Vault` rather than a plain origin:

```rust
let dest_vault = Arc::new(Vault::new("dest.toml".into())?);
let dest_as_origin = OriginVault::new(dest_vault);

source_vault.push(&root_id, &dest_as_origin).await?;
```

It's also reachable declaratively via `origin_config { type = "vault" }`, which points at another vault's own TOML config file:

```toml
# outer.toml
name = "outer-vault"

[origin_config]
type = "vault"
path = "inner.toml"
```

`OriginConfig::build`/`OriginConfig::from_file` open `inner.toml` as a full `Vault` (via `Vault::new`) and wrap it in an `OriginVault`, so `outer-vault`'s origin is `inner-vault` in its entirety — any error opening `inner.toml` (missing file, invalid TOML, bad origin config) propagates out of the outer build.

## Constants

`lib.rs` centralizes the string literals shared across origin implementations, instead of duplicating them (they were previously hardcoded independently in `object.rs`/`origin/command.rs`/`origin/http.rs`):

- `ROOT_ID` (`"/"`) and `ROOT_NAME` (`"##ROOT##"`) — the conventional root id/display name, used by `ObjectId::default`/`Object::root`/`Object::get_name`.
- `PLACEHOLDER_ID`, `PLACEHOLDER_NAME`, `PLACEHOLDER_SIZE`, `PLACEHOLDER_CONTENT_TYPE`, `PLACEHOLDER_MODIFIED`, `PLACEHOLDER_DESTINATION`, `PLACEHOLDER_KIND` — the bare keys (`"id"`, `"name"`, ...) that `OriginCommand`/`OriginHTTP` substitute into `{key}`-templated strings.
- `OBJECT_KIND_LEAF` (`"leaf"`) / `OBJECT_KIND_BRANCH` (`"branch"`) — the values `{kind}` takes.
- `UNKNOWN_CONTENT_TYPE` (`"unknown"`) — `OriginCommand`'s fallback when an object has no content type.
- `FETCH_CMD_FIELD`, `LIST_CMD_FIELD`, `GET_CMD_FIELD`, `PUT_CMD_FIELD`, `SEND_CMD_FIELD`, `DELETE_CMD_FIELD` — the `OriginConfig::Command`/`CmdType` field names (`"fetch_cmd"`, ...).

These are `pub`, so external code (e.g. a custom `Origin`) can reuse the same keys instead of re-hardcoding them.

## Status

135 unit tests (plus 40 doctests) covering `object` (including `ObjectId::default`/`is_root` and `Object::changed`), `error`, `config` (`VaultConfig::build` against real temp TOML files, one per origin variant including nested `vault`, plus `root_id` default/override and inner-vault error propagation; `OriginConfig::from_file` building each origin variant standalone, without a vault), `vault` (via mock `Origin`s, `Vault::new` against a real config file, `find` path resolution, cache behavior for `get`/`list`/`delete`, `pull`/`push` against an in-memory tree `Origin` — copying missing/changed objects, skipping unchanged ones, recursing into branches, and propagating unexpected errors — an end-to-end `push` between two real fs-backed vaults via `OriginVault`, and the `put`-contract regression tests described above), `origin::fs` (against real tempdirs), `origin::command` (against real shell commands like `echo`/`true`/`false`, including a `tokio::time::timeout`-guarded regression test for the `extra_vars` mutex deadlock described below), `origin::http` (against a mock server via `httpmock`, including `put`'s follow-up `get` and its failure/`NotFound` paths, plus authentication: every operation carrying the header — one mock per operation, so one that forgot would fail rather than pass unnoticed — no header at all when no credentials are configured, the 401 message, and each way a token can be resolved or refused), and `origin::vault` (`OriginVault` delegating `get`/`list`/`fetch`/`put`/`send`/`delete` to a real `Vault` backed by `OriginFileSystem`, including `NotFound` propagation).

Regression tests worth knowing about, all in `origin::command`: configured `extras` reaching `put_cmd`/`send_cmd`, `{kind}` distinguishing a leaf from a branch, per-object `meta.extra` beating a config default, and `{destination}` being refreshed rather than cached across two `put`s to different parents.

### `OriginCommand`'s `extra_vars` mutex

`extra_vars` moved from a plain field to a `futures::lock::Mutex<HashMap<String, String>>`, so `put` can record its `destination` argument without needing `&mut self` (the trait only gives it `&self`). The lock is **not** re-entrant: `put` used to hold its guard for its entire body, including a follow-up call to `self.get()` which itself locks `extra_vars` in `bootstrap_cmd_id` — a self-deadlock, since nothing else could ever release the outer guard. `put` now scopes its guard to just the `destination` insert, dropping it before doing anything else. `origin::command::tests::put_does_not_deadlock_on_extra_vars_mutex` wraps a `put` call in `tokio::time::timeout` to catch a regression as a test failure instead of a hung test run.

`bootstrap_cmd_object` (the `put`/`send` path) also has to lock it. It originally didn't, and interpolated only the *object's* `meta.extra` — so a config whose `put_cmd`/`send_cmd` referenced a shared `{root}` or `{helper}` ran with the literal braces still in the string. `bootstrap_cmd_id` did it correctly, which is why reads worked and only writes failed.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/vault/LICENSE-APACHE) or [MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/vault/LICENSE-MIT) at your option.
