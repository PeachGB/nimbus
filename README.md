# Nimbus

Nimbus is a generic sync abstraction: a tree of objects (a **vault**) whose actual
storage lives behind a pluggable **origin** — a local directory, an HTTP API, an
arbitrary shell command, or another vault. Syncing "a folder on disk" and "objects
behind a REST API" run through the exact same code path, because both are just
implementations of one `Origin` trait.

Two frontends drive it: [`nimbus-cli`](crates/cli/README.md), which runs a single
command or opens an interactive REPL, and [`nimbus-tui`](crates/tui/README.md), a
ranger-style file manager. [`nimbus-daemon`](crates/daemon/README.md) is the other
end of the wire: it serves a directory of vaults over HTTP, so one machine can hold
the data and the rest mount it through the `http` origin.

This repo is a Cargo workspace with six crates:

| Crate            | Status  | What it is                                              |
|------------------|---------|----------------------------------------------------------|
| `nimbus-vault`   | working | The core library: `Object`, `Vault`, `Origin` and its four implementations. |
| `nimbus-core`    | working | Session/vault-management logic (`App`) shared by nimbus's frontends — see [`crates/core/README.md`](crates/core/README.md). |
| `nimbus-creator` | working | An interactive Ratatui wizard that builds a `vault.toml`, embeddable from another frontend — see [`crates/creator/README.md`](crates/creator/README.md). |
| `nimbus-cli`     | working | One command per invocation, or an interactive REPL, built on `nimbus-core`/`nimbus-vault` — see [`crates/cli/README.md`](crates/cli/README.md). |
| `nimbus-tui`     | working | A ranger-style terminal file manager over vaults — see [`crates/tui/README.md`](crates/tui/README.md). |
| `nimbus-daemon`  | working | Serves a directory of vaults over HTTP, for the `http` origin to mount — see [`crates/daemon/README.md`](crates/daemon/README.md). |

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
# vault.toml — backed by an HTTP API (e.g. a nimbus-daemon)
name = "remote-vault"

[origin_config]
type = "http"
base_url   = "http://server:8080/v/photos"
list_url   = "/list/{id}"
fetch_url  = "/fetch/{id}"
get_url    = "/get/{id}"
put_url    = "/put/{id}"
send_url   = "/send/{id}"
delete_url = "/delete/{id}"

# optional; omit it and the origin sends no credentials
[origin_config.auth]
type = "bearer"
token_env = "NIMBUS_TOKEN"
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

The three binaries are published on crates.io and install independently — you only
need the ones you'll use:

```bash
cargo install nimbus-cli       # the REPL / one-shot command
cargo install nimbus-tui       # the file manager
cargo install nimbus-daemon    # the HTTP server
```

To use the library from your own crate:

```bash
cargo add nimbus-vault
```

Or build the whole workspace from a checkout — it's plain Cargo, no extra tooling:

```bash
cargo build --release
```

Build a single crate with `-p`, e.g. `cargo build -p nimbus-vault --release`.
Everything here needs **Rust 1.88** or newer (`rust-version` in the workspace
manifest); that floor comes from the dependencies, not from the code.

## CLI

`nimbus-cli` manages a set of named vaults plus a special local vault (your own
filesystem, named `LOCAL`), and moves objects between them. Give it a command and it
runs that one command and exits; give it nothing and it opens an interactive REPL:

```bash
nimbus-cli ls          # runs one command, exits with its status
nimbus-cli             # opens the REPL
```

Either way the session — the registry of vaults, the selected vault, and the
directory you were in — is saved on the way out and restored on the next run, so a
`nimbus-cli cd docs` followed by a `nimbus-cli ls` behaves like a shell.

The REPL's prompt carries your position (`nimbus />>`, `nimbus my-vault/docs>>`);
it's written `nimbus>` below for brevity:

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

## TUI

`nimbus-tui` is a ranger-style file manager over the same vaults: a list of
registered vaults, and inside each one a browsable object tree with size and
modified-time columns.

```bash
cargo run -p nimbus-tui
```

Arrow keys or `hjkl` to navigate, `Space` to mark, `y`/`d`/`p` to copy/cut/paste
(navigate to another vault before pasting to cross vaults), `a`/`t`/`c`/`x` to
create a directory, create a file, rename, and delete, `/` to filter, `s`/`S` to
sort, `n` to run the vault-creation wizard, and `?` for the full help overlay
(`c` prompts with the current name pre-filled, so renaming is an edit not a retype).
`:` opens a command line accepting the same commands as `nimbus-cli`.

Pressing `Enter` on a file fetches it, opens it with the OS default handler (or
`$EDITOR`), and writes any edit back to the object's origin on exit. `e` opens it
in `$EDITOR` regardless of what the OS would have picked, and `r` runs it as a
program in the terminal. See
[`crates/tui/README.md`](crates/tui/README.md) for the full key reference and the
known limits (operations block the event loop; there's no undo or trash).

## Serving vaults over HTTP

`nimbus-daemon` serves a directory of vault configs over HTTP, which is what the
`http` origin was waiting for: one machine holds the data, the rest mount it as a
vault and use the same `ls`/`get`/`put`/`push`/`pull` they'd use on a local folder.

```bash
nimbus-daemon --vaults ./vaults --bind 127.0.0.1:8080
```

Settings come from flags, then `~/.config/.nimbus/daemon_config.toml`, then the
defaults, in that order. The first run writes that file with the defaults in it, so
there's something to edit rather than a page of docs to consult:

```toml
vaults_path = "/srv/nimbus/vaults"   # required (or --vaults)
bind = "127.0.0.1:8080"              # default: loopback, never 0.0.0.0
read_only = false                    # refuse every write

[auth]                               # default: no authentication
type = "bearer"
token = "s3cr3t"
```

Every `*.toml` under `vaults_path` is opened as a vault and addressed by the `name`
inside it, at `/v/<name>/…`. The six routes mirror the URL templates `OriginHTTP`
uses, so a client only points `base_url` at `http://host:8080/v/<name>` and fills in
the default paths — the `http` example above is a complete client config for one.

It's a server, not a scheduler: it doesn't sync anything on its own. There's no TLS
either, so anything leaving the host wants a reverse proxy or a tunnel in front. See
[`crates/daemon/README.md`](crates/daemon/README.md) for the full API, the
authentication model, and how ids are validated at the boundary.

## Configuration locations

Every binary shares the same layout, all derived from `nimbus_vault::config_home()`
(`.nimbus` under the platform config dir, so `$XDG_CONFIG_HOME` is respected):

| Path | What |
|------|------|
| `<config>/.nimbus/cli_config.toml` | `default_local_vault`, `local_vault_path` |
| `<config>/.nimbus/daemon_config.toml` | `vaults_path`, `bind`, `read_only`, `[auth]` |
| `<config>/.nimbus/vaults/<name>.toml` | where the creator wizard saves new vault configs |
| `<state>/nimbus/session.toml` | the registry of `name → config path`, plus the vault and directory the last session ended in |

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

Working reference implementations of exactly this — plus an HTTP origin — live in
[`crates/cli/test/`](crates/cli/test/README.md): `command/cmd-vault.sh` turns a real
directory into a `command` origin, and `http/server.py` serves one over HTTP using
nothing but the Python standard library.

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

[`crates/cli/test/`](crates/cli/test/README.md) holds a hand-run vault config **per
origin type** — `fs`, `http` (served by a stdlib Python reference origin), `command`
(a POSIX-sh reference origin), and a vault wrapping another vault — so features can
be exercised against something other than a plain filesystem. Point
`XDG_STATE_HOME` at a scratch directory first and your real vault registry is left
alone:

```bash
export XDG_STATE_HOME=$(mktemp -d)
cargo run -p nimbus-cli
nimbus />> new crates/cli/test/fs/fs.toml
```

The configs carry **absolute** paths, so they need updating if the repo moves — see
that README for the per-origin walkthrough.

## Releasing

All six crates share one version, set once in `[workspace.package]` at the root and
inherited with `version.workspace = true`; the same goes for `edition`,
`rust-version`, `authors`, `license`, `repository`, and `homepage`. Each crate sets
its own `description`, `keywords`, `categories`, and `readme` — `readme` in
particular *cannot* be inherited, since an inherited one resolves against the
workspace root and every crate would ship the root README instead of its own.

Bumping a release means editing the one `version` at the root and the `version =`
on each inter-crate dependency (they're pinned exactly, e.g.
`nimbus-vault = { version = "0.2.0", path = "../vault" }` — the `path` is what a
workspace build uses, the `version` is what a crates.io consumer gets).

Publishing order is forced by the dependency graph, and each crate has to be live on
crates.io before the next one can be verified against it:

```bash
cargo publish -p nimbus-vault      # depends on nothing in-tree
cargo publish -p nimbus-core       # → vault
cargo publish -p nimbus-creator    # → vault
cargo publish -p nimbus-cli        # → core, creator
cargo publish -p nimbus-tui        # → core, creator, vault
cargo publish -p nimbus-daemon     # → vault
```

`cargo package --list -p <crate>` shows exactly what would be uploaded, which is
worth a look before the first publish of any crate — Cargo includes every tracked
file next to the manifest, not just `src/`.

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
[MIT license](LICENSE-MIT) at your option. All six crates declare the same
`MIT OR Apache-2.0`, and each carries its own copy of both texts so a crate stays
correctly licensed once it's unpacked from crates.io on its own.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
