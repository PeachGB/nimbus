# nimbus-cli

An interactive shell for managing **vaults** — commands to navigate, transfer, and sync objects across any configured origin (local filesystem, HTTP API, another vault, or a custom shell command).

A vault is a logical tree of objects (folders and files, conceptually) backed by a pluggable **origin** — the actual backend the data lives in. This CLI is a thin frontend over [`nimbus-core`](../core) (which owns the `App`/session state) and [`nimbus-vault`](../vault) (which implements the vault/origin model and does the actual work).

## How it works

`nimbus` starts a `rustyline`-backed REPL (tab completion for subcommand names and `cd`'s argument) rather than being invoked once per command. There's always a special **local vault** (named `LOCAL`) representing your own filesystem — every `put`/`get` moves data between that local vault and whichever remote vault you're working with. You never touch the OS filesystem directly; `put`/`get`'s local-side paths are checked to stay under the configured local root.

## Installation

```bash
cargo install --path crates/cli
```

## Configuration

### CLI settings (`~/.config/.nimbus/cli_config.toml`)

```toml
default_local_vault = true       # auto-register the local filesystem as a vault
local_vault_path = "/home/you"   # optional; defaults to $HOME
```

Set `default_local_vault = false` if you don't want the CLI touching your filesystem at all — `put`/`get` will then require every source/destination to be an explicit remote vault (`LOCAL` won't be registered).

### Vault definitions

Each vault you register (other than the automatic `LOCAL` one) is defined by its own `.toml` file — see [`crates/vault/README.md`](../vault/README.md) for the full `origin_config` shape (`fs`, `http`, `command`, `vault`). Register one with:

```
nimbus> new /path/to/vault.toml
```

or launch the interactive wizard (built on [`nimbus-creator`](../creator)) by running `new` with no path — it prompts for a name, root id, origin type, and that origin's fields, then writes and registers the resulting `vault.toml`.

## Session model

`nimbus` runs as a REPL: one process, and `select`/`cd`/etc. mutate in-memory state directly rather than re-parsing between invocations. Registered vaults (name → config path) are persisted to `~/.local/state/nimbus/session.toml` on `new` and on `exit`, so vaults registered in a previous session are still there next time `nimbus` launches.

## Commands

Inside the REPL, type a subcommand name:

### Navigation

```
ls                     # list contents of the current directory, or all registered vaults if none selected
vaults                 # list all registered vaults
select <vault>         # make <vault> the current vault
cd <path>              # change directory inside the current vault
cd <vault>/<path>      # select a vault and navigate in one step (when no vault is selected yet)
cd                     # (no argument) return to the root — deselects the current vault
```

### Registering vaults

```
new <config.toml>      # register a new vault from its config file
new                    # launch the interactive wizard to build and register one
```

### Moving data

```
put <path> [vault] [dest]
get <path> [vault] [dest]
```

`put` uploads something from your local filesystem into a vault. `get` downloads something from a vault into your local filesystem. `vault` defaults to whichever vault is currently selected; `dest` defaults to your current position (in the vault, for `put`; in your local directory, for `get`). Arguments are positional.

### Operating within a vault

```
cp <path> <destination> [vault]
mv <path> <destination> [vault]
delete <path> [vault] [-f | --force]
```

`cp`/`mv` copy or move an object within the same vault. `delete` refuses to remove a non-empty directory unless `--force` is given.

### Syncing with a remote origin

```
push [vault]           # send local changes to the vault's origin
pull [vault]            # bring changes from the origin into the vault
```

`push`/`pull` recursively sync an entire subtree, skipping objects that haven't changed (compared by metadata) rather than re-transferring everything on every run.

### Exiting

```
exit                   # save session state and quit
```

`Ctrl-C`/`Ctrl-D` also exit the REPL.

## Writing a custom origin

If none of the built-in origins (`fs`, `http`, `vault`) fit your backend, `command` lets you wire up arbitrary shell commands. Each operation gets its own command template with `{id}`, `{name}`, `{size}`, `{content_type}`, `{modified}`, `{destination}` (where applicable) substituted in, plus any custom `extras` you define — see [`crates/vault/README.md`](../vault/README.md) for the full reference:

- `list_cmd` / `get_cmd` must print JSON on stdout matching the object schema (`{"type": "leaf"|"branch", "id": "...", "name": "...", ...}`).
- `fetch_cmd` streams raw content bytes to stdout.
- `send_cmd` reads the payload from stdin.
- `put_cmd`/`delete_cmd` just need to succeed (exit code 0); stderr is captured for error reporting on failure.

## Notes

- `ls` always reflects the true current state of a vault's origin — it's never served from cache.
- Content (file bytes) is streamed, not buffered — large files don't get fully loaded into memory during transfer.
- `mv` and `push`/`pull`'s local↔remote transfers only proceed with the destructive step (delete, in `mv`'s case) after the copy has succeeded.
- Tab completion currently covers subcommand names and `cd`'s first argument only; other commands' path arguments aren't completed yet.
