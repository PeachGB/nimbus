# Nimbus

Nimbus is a generic sync abstraction: a tree of objects (a **vault**) whose actual
storage lives behind a pluggable **origin** — a local directory, an HTTP API, or an
arbitrary shell command. Syncing "a folder on disk" and "objects behind a REST API"
run through the exact same code path, because both are just implementations of one
`Origin` trait.

This repo is a Cargo workspace with four crates:

| Crate         | Status  | What it is                                              |
|---------------|---------|----------------------------------------------------------|
| `nimbus-vault`  | working | The core library: `Object`, `Vault`, `Origin` and its four implementations. |
| `nimbus-cli`    | WIP     | Command-line interface built on `nimbus-vault`. |
| `nimbus-daemon` | stub    | Background sync process (not yet implemented). |
| `nimbus-tui`    | stub    | Terminal UI frontend (not yet implemented). |

The rest of this document focuses mostly on `nimbus-vault`, since it's the only
crate with real functionality right now.

## The model

- **`Object`** — a node in the tree: `Leaf` (has content), `Branch` (has children),
  or `Root`. Objects only carry metadata (name, id, size, content type, modified
  time, plus a free-form `extra` map) — never raw bytes — so listing a tree never
  materializes its contents into memory.
- **`ObjectId`** — a newtype around `String`, opaque and origin-specific (a relative
  path for the filesystem origin, an arbitrary id for HTTP/command origins).
- **`Origin`** — the trait every backend implements:

  ```rust
  #[async_trait::async_trait]
  pub trait Origin: Send + Sync {
      async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream>;
      async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>>;
      async fn get(&self, id: &ObjectId) -> VaultResult<Object>;
      async fn put(&self, object: &Object) -> VaultResult<()>;
      async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
      async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
  }
  ```

  `fetch`/`send` are streaming (`ByteStream = BoxStream<'static, VaultResult<Bytes>>`) —
  content moves in chunks, it's never buffered whole into RAM.

- **`Vault`** — owns one `Origin` plus an in-memory metadata cache
  (`Mutex<HashMap<ObjectId, Object>>`). `get`/`list` populate the cache; `list`
  always re-hits the origin (it's the source of truth) while refreshing the cache.
  `find(path)` resolves a `/`-separated path to an `ObjectId` by walking the tree
  one `list` call per component.

Four built-in origins ship in `nimbus-vault`:

- `OriginFileSystem` (`fs`) — a directory on disk, via `tokio::fs`.
- `OriginHTTP` (`http`) — any REST-ish API, with a `{id}`-templated URL per operation.
- `OriginCommand` (`command`) — a shell command per operation; the universal escape
  hatch (see below).
- `OriginVault` (`vault`) — another `Vault`, wrapped so it can act as an origin
  in its own right (see [Using a vault as an origin](#using-a-vault-as-an-origin)).

A vault is fully described by a TOML file, deserialized into `VaultConfig` /
`OriginConfig`:

```toml
# vault.toml — backed by a local directory
name = "my-vault"

[origin_config]
type = "fs"
root = "/srv/data"
```

```toml
# vault.toml — backed by arbitrary shell commands
name = "cmd-vault"

[origin_config]
type = "command"
list_cmd   = "ls"
fetch_cmd  = "cat {id}"
get_cmd    = "stat {id}"
put_cmd    = "touch {id}"
send_cmd   = "touch {id}"
delete_cmd = "rm {id}"
```

```toml
# vault.toml — backed by another vault
name = "outer-vault"

[origin_config]
type = "vault"
path = "inner.toml"
```

```rust
use nimbus_vault::vault::Vault;

let vault = Vault::new("vault.toml".into())?;
let root = vault.find("/".into()).await?;
let children = vault.list(root).await?;
```

## Installation

```bash
cargo build --release
```

This is a plain Cargo workspace — no extra tooling is required. Build a single
crate with `-p`, e.g. `cargo build -p nimbus-vault --release`.

## CLI

`nimbus-cli` is under active development; the command surface defined in
`crates/cli/src/main.rs` is:

```
nimbus ls     <VAULT_NAME>                       # list vaults, or a vault's contents
nimbus new    <PATH>                             # create a new vault from a config
nimbus cd     <PATH>                             # change into a vault or object
nimbus put    <PATH> [NAME]                      # put an object into the vault
nimbus get    <NAME> [DESTINATION_PATH]           # get an object out of the vault
nimbus cp     <NAME> <DESTINATION_NAME>           # copy an object within a vault
nimbus mv     <NAME> <DESTINATION_NAME>           # move an object within a vault
nimbus origin <VAULT_NAME> <ORIGIN>               # set a vault's origin
nimbus sync                                       # sync the vault with its origin
```

The argument parsing is wired up, but most subcommands are still stubs
(`todo!()`) — treat the CLI as scaffolding rather than a finished tool for now.
`nimbus-daemon` and `nimbus-tui` are placeholder binaries with no logic yet.

## Writing a custom origin: `OriginCommand`

`OriginCommand` is the escape hatch for anything that isn't a plain filesystem or
HTTP API: it shells out to a user-configured command per operation. `list`/`get`
expect the command's stdout to be JSON matching the `Object` schema; `fetch`
streams stdout as the payload; `send` streams the payload to the command's stdin.
Templates support `{id}` plus any `extras` you define.

```toml
# origin.toml — no vault needed, just an origin
type = "command"
list_cmd   = "ls {root}"
fetch_cmd  = "cat {root}/{id}"
get_cmd    = "stat {root}/{id}"
put_cmd    = "touch {root}/{id}"
send_cmd   = "tee {root}/{id}"
delete_cmd = "rm {root}/{id}"

[extras]
root = "/srv/data"
```

```rust
use nimbus_vault::config::OriginConfig;

// builds just the Origin, without a name/root_id/Vault wrapper —
// useful for tooling that talks to an origin directly, or as the
// `remote` argument to Vault::pull/Vault::push.
let origin = OriginConfig::from_file("origin.toml".into())?;
```

`OriginConfig::build` takes `self` by value rather than `&self`, so building an
origin moves each variant's fields (command strings, URLs, the filesystem root,
...) straight into the `Origin` it constructs instead of cloning them — `build`
consumes the config, it doesn't just read it.

Any program that can read arguments, print JSON, and read/write stdio can be an
origin — a database CLI, a `curl` wrapper, a custom binary, anything.

## Using a vault as an origin

`OriginVault` wraps an `Arc<Vault>` and implements `Origin` by forwarding every
call to the wrapped vault's own method of the same name. That means one `Vault`
can act as the `remote` for another vault's `push`/`pull`, so two vaults can sync
directly with each other:

```rust
use nimbus_vault::origin::vault::OriginVault;

let dest_vault = Arc::new(Vault::new("dest.toml".into())?);
let dest_as_origin = OriginVault::new(dest_vault);

source_vault.push(&root_id, &dest_as_origin).await?;
```

It's also reachable declaratively, by pointing an `origin_config` at another
vault's own config file:

```toml
# outer.toml
name = "outer-vault"

[origin_config]
type = "vault"
path = "inner.toml"
```

Building `outer.toml` opens `inner.toml` as a full `Vault` (via `Vault::new`) and
wraps it in an `OriginVault`, so any error opening the inner vault (missing file,
invalid TOML, bad origin config) propagates straight out of the outer build.

## Syncing between origins

`Vault::pull(id, remote)` / `Vault::push(id, remote)` recursively sync the subtree
at `id` between the vault's own origin and any other `&dyn Origin` — a plain
origin, or another `Vault` wrapped in `OriginVault` — using `Object::changed` (a
metadata hash comparison) to skip objects that haven't changed:

```rust
// bring the vault's local origin up to date with `remote`
vault.pull(&root_id, remote.as_ref()).await?;

// push the vault's local subtree out to `remote`
vault.push(&root_id, remote.as_ref()).await?;
```

## Design principles

- **Lazy loading** — `Object` only ever holds metadata; content is fetched on
  demand via `fetch`, so listing a huge tree doesn't pull its contents into memory.
- **Streaming, not buffering** — `fetch`/`send` move payloads as a `ByteStream` of
  chunks, never as one big in-memory blob.
- **Origin-agnostic sync** — `pull`/`push` are written entirely against the
  `Origin` trait, so the same sync logic works between any two backends: disk,
  HTTP, shell command, another vault, or a mix of the four.

## License

No license file is currently included in this repository. Until one is added,
treat the code as all-rights-reserved.
