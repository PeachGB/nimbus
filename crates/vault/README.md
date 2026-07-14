# vault

Library crate for the Nimbus CLI/TUI. Abstracts a remote or local "origin" — a filesystem, a shell command, an HTTP API — behind a single tree-like `Vault` structure, similar to a filesystem, regardless of where the underlying objects actually live.

## What's here

- **`vault.rs`** — `Vault`: holds a name, an `Arc<dyn Origin>`, a local `Object` cache (`Mutex<HashMap<ObjectId, Object>>`), and a single root `ObjectId`. `Vault::new(path)` reads a `VaultConfig` from a TOML file at `path` and bootstraps the origin and root id from it. `find(path)` walks a filesystem-style `PathBuf` component by component, calling `list` at each level and matching child names, to resolve it to an `ObjectId` starting from the root. `get` serves from the cache when present, falling back to the origin and caching the result; `list` always hits the origin but caches every child it returns (and updates the parent's cached child-id list, if the parent is itself cached as a `Branch`/`Root`); `delete` evicts an id from the cache. `fetch`/`send` stream payloads straight through the origin and are never cached. `put(object, destination)` writes `object` under `destination` and caches (and returns) the `Object` the origin reports back — not the input — since an origin isn't required to rename `object` in place (see [The `put` contract](#the-put-contract)); on failure, nothing is cached. `pull(id, remote)`/`push(id, remote)` recursively sync `id`'s subtree between this vault's origin and an arbitrary `&dyn Origin`, threading `put`'s returned `Object` through to the following `send` — see [Syncing between origins](#syncing-between-origins) below.
- **`config.rs`** — `VaultConfig`: the on-disk shape of a vault (`name`, an optional `root_id` defaulting to `"/"`, plus an `origin_config`), and `OriginConfig`, a tagged enum (`type = "fs" | "command" | "http" | "vault"`) describing how to build the origin declaratively. `VaultConfig::build(path)` reads and parses the TOML file and returns the vault's name, root id, and the matching `Box<dyn Origin>`. `OriginConfig::from_file(path)` builds just the `Box<dyn Origin>` from a TOML file containing only the `origin_config` shape (no `name`/`root_id`), for callers that want to talk to an origin directly without opening a `Vault` — see [Building an origin without a vault](#building-an-origin-without-a-vault). `OriginConfig::build(self)` takes `self` by value rather than `&self`, so it moves each variant's fields straight into the `Origin` it constructs instead of cloning them.
- **`object.rs`** — `Object` (`Leaf` / `Branch` / `Root` variants), `ObjectId` (newtype around `String`, defaults to `ROOT_ID` (`"/"`), with `is_root()`), `Metadata` (size, content type, modified time, free-form `extra` map, plus `hash_value()` for a stable content hash). `Object::push` appends a child id onto a `Branch`/`Root`; `Object::with_id` overwrites an object's id in place (used by origins that rename an object on `put`, e.g. `OriginFileSystem`). `Object::get_name()` returns `ROOT_NAME` (`"##ROOT##"`) for `Root`, which has no real name. `Object::changed(remote)` compares `hash_value()` on both sides' metadata to detect drift between a local and remote copy of the same object, returning `false` if either side has no metadata (e.g. `Root`).
- **`origin/mod.rs`** — the `Origin` trait (`fetch`, `list`, `get`, `put`, `send`, `delete`, plus `Send + Sync`) that every backend implements.
- **`origin/fs.rs`** — `OriginFileSystem`: origin backed by a directory on disk. `ObjectId`s are relative paths under `root`. `put` resolves the object's path as `{destination}/{name}`, renames the object in place (via `Object::with_id`) to that path, and creates the file/directory.
- **`origin/command.rs`** — `OriginCommand`: origin backed by shell commands, one per operation (`fetch_cmd`, `list_cmd`, `get_cmd`, `put_cmd`, `send_cmd`, `delete_cmd`). Commands are `{placeholder}`-templated with the object id, name, and metadata, plus arbitrary `extra_vars` (guarded by an internal `futures::lock::Mutex`, since `put` needs to set a `destination` var without requiring `&mut self`); `list`/`get` expect the command's stdout to be JSON matching `Object`. `put` runs `put_cmd`, then re-`get`s `"{destination}/{name}"` to return the stored `Object` — it does **not** rename the input object in place, unlike `OriginFileSystem`.
- **`origin/http.rs`** — `OriginHTTP`: origin backed by a REST-ish HTTP API. `base_url` plus a `{id}`-templated path per operation (`fetch_url`, `list_url`, `get_url`, `put_url`, `send_url`, `delete_url`). `get`/`list` are `GET`s deserialized as JSON; `fetch` streams the response body; `put` `PUT`s the `Object` to `{put_url}/{destination}`, then re-`get`s `"{destination}/{name}"` to return the stored `Object` (again, without mutating the input); `send` `PUT`s the payload stream as the request body; `delete` is a `DELETE`. Any non-2xx response becomes a `VaultError`, with 404 mapped to `NotFound`.
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
- `PLACEHOLDER_ID`, `PLACEHOLDER_NAME`, `PLACEHOLDER_SIZE`, `PLACEHOLDER_CONTENT_TYPE`, `PLACEHOLDER_MODIFIED`, `PLACEHOLDER_DESTINATION` — the bare keys (`"id"`, `"name"`, ...) that `OriginCommand`/`OriginHTTP` substitute into `{key}`-templated strings.
- `UNKNOWN_CONTENT_TYPE` (`"unknown"`) — `OriginCommand`'s fallback when an object has no content type.
- `FETCH_CMD_FIELD`, `LIST_CMD_FIELD`, `GET_CMD_FIELD`, `PUT_CMD_FIELD`, `SEND_CMD_FIELD`, `DELETE_CMD_FIELD` — the `OriginConfig::Command`/`CmdType` field names (`"fetch_cmd"`, ...).

These are `pub`, so external code (e.g. a custom `Origin`) can reuse the same keys instead of re-hardcoding them.

## Commands

```bash
cargo check -p vault
cargo test -p vault
cargo clippy -p vault -- -D warnings
cargo fmt -p vault
```

## Status

108 unit tests (plus 34 doctests) covering `object` (including `ObjectId::default`/`is_root` and `Object::changed`), `error`, `config` (`VaultConfig::build` against real temp TOML files, one per origin variant including nested `vault`, plus `root_id` default/override and inner-vault error propagation; `OriginConfig::from_file` building each origin variant standalone, without a vault), `vault` (via mock `Origin`s, `Vault::new` against a real config file, `find` path resolution, cache behavior for `get`/`list`/`delete`, `pull`/`push` against an in-memory tree `Origin` — copying missing/changed objects, skipping unchanged ones, recursing into branches, and propagating unexpected errors — an end-to-end `push` between two real fs-backed vaults via `OriginVault`, and the `put`-contract regression tests described above), `origin::fs` (against real tempdirs), `origin::command` (against real shell commands like `echo`/`true`/`false`, including a `tokio::time::timeout`-guarded regression test for the `extra_vars` mutex deadlock described below), `origin::http` (against a mock server via `httpmock`, including `put`'s follow-up `get` and its failure/`NotFound` paths), and `origin::vault` (`OriginVault` delegating `get`/`list`/`fetch`/`put`/`send`/`delete` to a real `Vault` backed by `OriginFileSystem`, including `NotFound` propagation).

### `OriginCommand`'s `extra_vars` mutex

`extra_vars` moved from a plain field to a `futures::lock::Mutex<HashMap<String, String>>`, so `put` can record its `destination` argument without needing `&mut self` (the trait only gives it `&self`). The lock is **not** re-entrant: `put` used to hold its guard for its entire body, including a follow-up call to `self.get()` which itself locks `extra_vars` in `bootstrap_cmd_id` — a self-deadlock, since nothing else could ever release the outer guard. `put` now scopes its guard to just the `destination` check/insert, dropping it before doing anything else. `origin::command::tests::put_does_not_deadlock_on_extra_vars_mutex` wraps a `put` call in `tokio::time::timeout` to catch a regression as a test failure instead of a hung test run.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
