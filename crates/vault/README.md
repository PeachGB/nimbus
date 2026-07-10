# vault

Library crate for the Nimbus CLI/TUI. Abstracts a remote or local "origin" — a filesystem, a shell command, an HTTP API — behind a single tree-like `Vault` structure, similar to a filesystem, regardless of where the underlying objects actually live.

## What's here

- **`vault.rs`** — `Vault`: holds a name, an `Arc<dyn Origin>`, a local `Object` cache (`Mutex<HashMap<ObjectId, Object>>`), and a single root `ObjectId`. `Vault::new(path)` reads a `VaultConfig` from a TOML file at `path` and bootstraps the origin and root id from it. `find(path)` walks a filesystem-style `PathBuf` component by component, calling `list` at each level and matching child names, to resolve it to an `ObjectId` starting from the root. `get` serves from the cache when present, falling back to the origin and caching the result; `list` always hits the origin but caches every child it returns (and updates the parent's cached child-id list, if the parent is itself cached as a `Branch`/`Root`); `put` caches the written object; `delete` evicts it from the cache. `fetch`/`send` stream payloads straight through the origin and are never cached. `pull(id, remote)`/`push(id, remote)` recursively sync `id`'s subtree between this vault's origin and an arbitrary `&dyn Origin` — see [Syncing between origins](#syncing-between-origins) below.
- **`config.rs`** — `VaultConfig`: the on-disk shape of a vault (`name`, an optional `root_id` defaulting to `"/"`, plus an `origin_config`), and `OriginConfig`, a tagged enum (`type = "fs" | "command" | "http"`) describing how to build the origin declaratively. `VaultConfig::build(path)` reads and parses the TOML file and returns the vault's name, root id, and the matching `Box<dyn Origin>`. `OriginConfig::from_file(path)` builds just the `Box<dyn Origin>` from a TOML file containing only the `origin_config` shape (no `name`/`root_id`), for callers that want to talk to an origin directly without opening a `Vault` — see [Building an origin without a vault](#building-an-origin-without-a-vault).
- **`object.rs`** — `Object` (`Leaf` / `Branch` / `Root` variants), `ObjectId` (newtype around `String`, defaults to `"/"`, with `is_root()`), `Metadata` (size, content type, modified time, free-form `extra` map, plus `hash_value()` for a stable content hash). `Object::push` appends a child id onto a `Branch`/`Root`. `Object::changed(remote)` compares `hash_value()` on both sides' metadata to detect drift between a local and remote copy of the same object, returning `false` if either side has no metadata (e.g. `Root`).
- **`origin/mod.rs`** — the `Origin` trait (`fetch`, `list`, `get`, `put`, `send`, `delete`) that every backend implements.
- **`origin/fs.rs`** — `OriginFileSystem`: origin backed by a directory on disk. `ObjectId`s are relative paths under `root`.
- **`origin/command.rs`** — `OriginCommand`: origin backed by shell commands, one per operation (`fetch_cmd`, `list_cmd`, `get_cmd`, `put_cmd`, `send_cmd`, `delete_cmd`). Commands are `{placeholder}`-templated with the object id, name, and metadata, plus arbitrary `extra_vars`; `list`/`get` expect the command's stdout to be JSON matching `Object`.
- **`origin/http.rs`** — `OriginHTTP`: origin backed by a REST-ish HTTP API. `base_url` plus a `{id}`-templated path per operation (`fetch_url`, `list_url`, `get_url`, `put_url`, `send_url`, `delete_url`). `get`/`list` are `GET`s deserialized as JSON; `fetch` streams the response body; `put` `PUT`s the `Object` as a JSON body; `send` `PUT`s the payload stream as the request body; `delete` is a `DELETE`. Any non-2xx response becomes a `VaultError`, with 404 mapped to `NotFound`.
- **`error.rs`** — `VaultError` (`thiserror`-based) / `VaultResult<T>`, the error type used across the crate. `NotFound` carries the id/url/name that wasn't found; `Io`, `Json`, `Toml`, and `HTTP` wrap the corresponding std/serde/toml/reqwest errors via `#[from]`.

## Origin trait

```rust
#[async_trait::async_trait]
pub trait Origin {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream>;
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>>;
    async fn get(&self, id: &ObjectId) -> VaultResult<Object>;
    async fn put(&self, object: &Object) -> VaultResult<()>;
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
    async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
}
```

`ByteStream` is a boxed, pinned stream of `VaultResult<Bytes>` — used for both reading (`fetch`) and writing (`send`) object payloads without buffering the whole thing in memory.

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
2. If the object is missing, or present but `Object::changed` reports the metadata hashes diverge, `put` the object on the destination and — for `Leaf`s only — `fetch` the payload from the source and `send` it to the destination.
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

## Commands

```bash
cargo check -p vault
cargo test -p vault
cargo clippy -p vault -- -D warnings
cargo fmt -p vault
```

## Status

96 unit tests covering `object` (including `ObjectId::default`/`is_root` and `Object::changed`), `error`, `config` (`VaultConfig::build` against real temp TOML files, one per origin variant, plus `root_id` default/override; `OriginConfig::from_file` building each origin variant standalone, without a vault), `vault` (via a mock `Origin`, `Vault::new` against a real config file, `find` path resolution, cache behavior for `get`/`list`/`put`/`delete`, and `pull`/`push` against an in-memory tree `Origin` — copying missing/changed objects, skipping unchanged ones, recursing into branches, and propagating unexpected errors), `origin::fs` (against real tempdirs), `origin::command` (against real shell commands like `echo`/`true`/`false`), and `origin::http` (against a mock server via `httpmock`).
