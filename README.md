# Nimbus

Nimbus is a generic sync abstraction: a tree of objects (a **vault**) whose actual
storage lives behind a pluggable **origin** — a local directory, an HTTP API, an
arbitrary shell command, or another vault. Syncing "a folder on disk" and "objects
behind a REST API" run through the exact same code path, because both are just
implementations of one `Origin` trait.

Two frontends drive it: [`nimbus-cli`](crates/cli/README.md), an interactive REPL,
and [`nimbus-tui`](crates/tui/README.md), a ranger-style file manager.

This repo is a Cargo workspace with six crates:

| Crate            | Status  | What it is                                              |
|------------------|---------|----------------------------------------------------------|
| `nimbus-vault`   | working | The core library: `Object`, `Vault`, `Origin` and its four implementations. |
| `nimbus-core`    | working | Session/vault-management logic (`App`) shared by nimbus's frontends — see [`crates/core/README.md`](crates/core/README.md). |
| `nimbus-creator` | working | An interactive Ratatui wizard that builds a `vault.toml`, embeddable from another frontend — see [`crates/creator/README.md`](crates/creator/README.md). |
| `nimbus-cli`     | working | An interactive REPL built on `nimbus-core`/`nimbus-vault` — see [`crates/cli/README.md`](crates/cli/README.md). |
| `nimbus-tui`     | working | A ranger-style terminal file manager over vaults — see [`crates/tui/README.md`](crates/tui/README.md). |
| `nimbus-daemon`  | stub    | Background sync process (not yet implemented) — see [`crates/daemon/README.md`](crates/daemon/README.md). |

The rest of this document focuses mostly on `nimbus-vault`, since it's the library
every other crate builds on. See [`crates/cli/README.md`](crates/cli/README.md) for
`nimbus-cli`'s own commands, configuration, and session-persistence model,
[`crates/tui/README.md`](crates/tui/README.md) for the file manager's keys and
behaviour, and [`crates/core/README.md`](crates/core/README.md) for the `App` logic
both frontends are built on.

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
      async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object>;
      async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
      async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
  }
  ```

  `fetch`/`send` are streaming (`ByteStream = BoxStream<'static, VaultResult<Bytes>>`) —
  content moves in chunks, it's never buffered whole into RAM.

  `put(object, destination)` writes `object` under `destination` and returns the
  `Object` as it now exists at the origin. Implementations are allowed, but not
  required, to rename `object` in place (`OriginFileSystem` does, via
  `Object::with_id`; `OriginHTTP`/`OriginCommand` don't) — callers should always
  use the *returned* `Object` for anything downstream, never assume `object` was
  mutated. `Vault::put`/`Vault::pull`/`Vault::push` follow this rule and only
  cache on success.

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

`nimbus-cli` starts an interactive REPL that manages a set of named vaults plus a
special local vault (your own filesystem, named `LOCAL`), and moves objects
between them:

```
nimbus> ls                                   # list the current vault's cwd, or all known vaults if none selected
nimbus> vaults                               # list all known vaults
nimbus> select <VAULT>                       # make <VAULT> the current vault
nimbus> new <CONFIG_PATH>                    # register a vault from its TOML config file
nimbus> new                                  # launch an interactive wizard to build and register one
nimbus> forget <VAULT>                       # stop tracking a vault (its config and data are untouched)
nimbus> cd <PATH>                            # change directory inside the current vault
nimbus> put <PATH> [VAULT] [DEST]            # copy a real filesystem path into a vault
nimbus> get <PATH> [VAULT] [DEST]            # copy an object out to the local vault
nimbus> mkdir <PATH> [VAULT]                 # create a directory
nimbus> touch <PATH> [VAULT]                 # create an empty file
nimbus> rename <PATH> <NEW_NAME> [VAULT]     # rename in place (a name, not a path)
nimbus> cp <PATH> <DESTINATION> [VAULT]      # copy an object, within or across vaults
nimbus> mv <PATH> <DESTINATION> [VAULT]      # move an object, within or across vaults
nimbus> delete <PATH> [VAULT] [--force]      # delete an object
nimbus> push [VAULT]                         # sync the local vault out to a vault
nimbus> pull [VAULT]                         # sync a vault into the local vault
nimbus> exit                                 # save session state and quit
```

Every path argument is a `vault:path` spec: bare paths are relative to the current
directory, a leading `/` is absolute within the current vault, and a `vault:` prefix
addresses another vault from its root. That prefix is what makes `cp`/`mv` cross
vaults — including between different origin types — and what distinguishes a vault
from a directory of the same name:

```
nimbus> cp notes.txt backup:/inbox           # into a directory, keeping the name
nimbus> cp notes.txt backup:/inbox/copy.txt  # under a new name
```

See [`crates/cli/README.md`](crates/cli/README.md) for the full command reference,
the local-vault security boundary, and session persistence — the REPL logic itself
(the `App` type) lives in [`nimbus-core`](crates/core/README.md), so `nimbus-cli` is
a fairly thin binary over it. `nimbus new` with no path launches an interactive
vault-builder wizard from [`nimbus-creator`](crates/creator/README.md).
`nimbus-daemon` is still a placeholder binary with no logic yet.

## TUI

`nimbus-tui` is a ranger-style file manager over the same vaults: a list of
registered vaults, and inside each one a browsable object tree with size and
modified-time columns.

```bash
cargo run -p nimbus-tui
```

![The vault list: every registered vault, with the root-level keybinding hints along the bottom.](docs/screenshots/vault-list.png)

![Browsing a vault: directories first, then files, each with a size and modified time.](docs/screenshots/object-browser.png)

Arrow keys or `hjkl` to navigate, `Space` to mark, `y`/`d`/`p` to copy/cut/paste
(navigate to another vault before pasting to cross vaults), `a`/`t`/`r`/`x` to
create a directory, create a file, rename, and delete, `/` to filter, `s`/`S` to
sort, `n` to run the vault-creation wizard, and `?` for the full help overlay
(`r` prompts with the current name pre-filled, so renaming is an edit not a retype).
`:` opens a command line accepting the same commands as `nimbus-cli`.

Pressing `Enter` on a file fetches it, opens it with the OS default handler (or
`$EDITOR`), and writes any edit back to the object's origin on exit. See
[`crates/tui/README.md`](crates/tui/README.md) for the full key reference and the
known limits (operations block the event loop; there's no undo or trash).

## Configuration locations

Both frontends share the same layout, all derived from `nimbus_vault::config_home()`
(`.nimbus` under the platform config dir, so `$XDG_CONFIG_HOME` is respected):

| Path | What |
|------|------|
| `<config>/.nimbus/cli_config.toml` | `default_local_vault`, `local_vault_path` |
| `<config>/.nimbus/vaults/<name>.toml` | where the creator wizard saves new vault configs |
| `<state>/nimbus/session.toml` | the registry of `name → config path` |

A vault config can live anywhere — `new <path>` registers it from wherever it is.
The `vaults/` directory is just the wizard's default, so created vaults stay
together and can be found again.

## Writing a custom origin: `OriginCommand`

`OriginCommand` is the escape hatch for anything that isn't a plain filesystem or
HTTP API: it shells out to a user-configured command per operation. `list`/`get`
expect the command's stdout to be JSON matching the `Object` schema; `fetch`
streams stdout as the payload; `send` streams the payload to the command's stdin;
`put` runs `put_cmd` and then re-`get`s `"{destination}/{name}"` to return the
stored `Object` (it does not rename the object you passed in).

Every template gets `{id}` plus any `extras` you define. `put_cmd`/`send_cmd`
additionally get `{name}`, `{size}`, `{content_type}`, `{modified}`, `{kind}`
(`leaf` or `branch` — without it a `put_cmd` can't tell whether to create a file or
a directory), and `{destination}`, the parent id, refreshed on every call.

`extra_vars` lives behind an internal `futures::lock::Mutex` so `put` can record
`destination` there without needing `&mut self`; it's scoped to just that
read-modify-write, since holding it any longer would deadlock against `put`'s own
follow-up `get` call (which locks the same mutex).

```toml
# origin.toml — no vault needed, just an origin
type = "command"
list_cmd   = "my-helper list {root} {id}"
fetch_cmd  = "cat {root}/{id}"
get_cmd    = "my-helper get {root} {id}"
put_cmd    = "my-helper put {root} {destination} {name} {kind}"
send_cmd   = "tee {root}/{id}"
delete_cmd = "rm -rf {root}/{id}"

[extras]
root = "/srv/data"
```

A working reference implementation of exactly this — plus an HTTP origin — lives in
[`dev/testenv/`](dev/testenv/README.md).

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
metadata hash comparison) to skip objects that haven't changed. When an object
needs syncing, they `put` it and then `send` its payload to whatever `Object`
`put` returned, not the pre-`put` object — see the `put` contract above:

```rust
// bring the vault's local origin up to date with `remote`
vault.pull(&root_id, remote.as_ref()).await?;

// push the vault's local subtree out to `remote`
vault.push(&root_id, remote.as_ref()).await?;
```

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

[`dev/testenv/`](dev/testenv/README.md) builds a throwaway sandbox with **one vault
per origin type** — `fs`, `http` (served by a stdlib Python reference origin),
`command` (a POSIX-sh reference origin), and a vault wrapping another vault — so
features can be exercised against something other than a plain filesystem:

```bash
dev/testenv/testenv.sh up      # build the sandbox and start the HTTP origin
dev/testenv/testenv.sh tui     # run the TUI against it
dev/testenv/testenv.sh clean   # tear it all down
```

It redirects `XDG_STATE_HOME`/`XDG_CONFIG_HOME` inside the sandbox, so your real
vault registry is never read or written and the whole thing can be wiped freely.

## Design principles

- **Lazy loading** — `Object` only ever holds metadata; content is fetched on
  demand via `fetch`, so listing a huge tree doesn't pull its contents into memory.
- **Streaming, not buffering** — `fetch`/`send` move payloads as a `ByteStream` of
  chunks, never as one big in-memory blob.
- **Origin-agnostic sync** — `pull`/`push` are written entirely against the
  `Origin` trait, so the same sync logic works between any two backends: disk,
  HTTP, shell command, another vault, or a mix of the four.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
