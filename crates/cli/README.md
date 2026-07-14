# nimbus-cli

A command-line app for managing **vaults** — shell-like commands to navigate, transfer, and sync objects across any configured origin (local filesystem, HTTP API, or a custom shell command).

A vault is a logical tree of objects (folders and files, conceptually) backed by a pluggable **origin** — the actual backend the data lives in. The origin can be your own disk, a REST API, or a script you wrote to talk to whatever system you want. This CLI is built on [`nimbus-vault`](../vault), the library that implements the vault/origin model and does the actual work; the CLI is just the command-line surface over it.

## How it works

The CLI treats every vault the same way, regardless of what backs it. There's always a special **local vault** (`##LOCAL##`) that represents your own filesystem — every `put`/`get` moves data between that local vault and whichever remote vault you're working with. You never touch the OS filesystem directly; everything goes through the same vault abstraction.

## Installation

```bash
cd cli
cargo install --path .
```

## Configuration

### CLI settings (`~/.config/.nimbus/cli_config.toml`)

```toml
default_local_vault = true       # auto-register the local filesystem as a vault
local_vault_path = "/home/you"   # optional; defaults to $HOME
```

Set `default_local_vault = false` if you don't want the CLI touching your filesystem at all — `put`/`get` will then require every source/destination to be an explicit remote vault.

### Vault definitions

Each vault you register (other than the automatic local one) is defined by its own `.toml` file:

```toml
name = "backup"
root_id = "/"

[origin_config]
type = "fs"
root = "/mnt/backup"
```

```toml
name = "api-drive"
root_id = "/"

[origin_config]
type = "http"
base_url = "https://api.example.com"
list_url = "/objects/{id}/children"
get_url = "/objects/{id}"
fetch_url = "/objects/{id}/content"
put_url = "/objects/{id}"
send_url = "/objects/{id}/content"
delete_url = "/objects/{id}"
```

```toml
name = "custom-backend"
root_id = "/"

[origin_config]
type = "command"
list_cmd = "my-lister {id}"
get_cmd = "my-stat {id}"
fetch_cmd = "my-cat {id}"
put_cmd = "my-create {destination} {name}"
send_cmd = "my-write {id}"
delete_cmd = "my-rm {id}"
```

Register a vault with:

```bash
nimbus new /path/to/vault.toml
```

## Session model

Every invocation of `nimbus` is a separate process — there's no long-running shell. State (which vault is selected, where you are inside it) is saved to `~/.local/state/nimbus/session.toml` after each command and reloaded on the next one, so `cd`, `select`, etc. persist across separate `nimbus` calls the same way they would in an interactive shell.

## Commands

### Navigation

```bash
nimbus vaults                  # list all registered vaults
nimbus select <vault>          # enter a vault
nimbus ls                      # list contents (or list vaults, if none selected)
nimbus cd <path>                # change directory inside the current vault
nimbus cd <vault>/<path>        # select a vault and navigate in one step
nimbus cd ..                    # go up a level
```

### Registering vaults

```bash
nimbus new <config.toml>        # register a new vault from its config file
```

### Moving data

```bash
nimbus put <local-path> [--vault X] [--dest Y]
nimbus get <remote-path> [--vault X] [--dest Y]
```

`put` uploads something from your local filesystem into a vault. `get` downloads something from a vault into your local filesystem. Both default `--vault` to whichever vault is currently selected, and `--dest` to your current position (in the vault, for `put`; in your local directory, for `get`).

### Operating within a vault

```bash
nimbus cp <path> <destination> [--vault X]
nimbus mv <path> <destination> [--vault X]
nimbus delete <path> [--vault X] [--force]
```

`cp`/`mv` copy or move an object within the same vault. `delete` refuses to remove a non-empty directory unless `--force` is given.

### Syncing with a remote origin

```bash
nimbus push [vault]             # send local changes to the vault's origin
nimbus pull [vault]             # bring changes from the origin into the vault
```

`push`/`pull` recursively sync an entire subtree, skipping objects that haven't changed (compared by metadata) rather than re-transferring everything on every run.

## Writing a custom origin

If none of the built-in origins (`fs`, `http`) fit your backend, `command` lets you wire up arbitrary shell commands. Each operation gets its own command template with `{id}`, `{name}`, `{size}`, `{content_type}`, `{modified}`, `{destination}` (where applicable) substituted in, plus any custom `extras` you define:

- `list_cmd` / `get_cmd` must print JSON on stdout matching the object schema (`{"type": "leaf"|"branch", "id": "...", "name": "...", ...}`).
- `fetch_cmd` streams raw content bytes to stdout.
- `send_cmd` reads the payload from stdin.
- `put_cmd`/`delete_cmd` just need to succeed (exit code 0); stderr is captured for error reporting on failure.

This is the escape hatch for backends with no existing origin — wrap them in a couple of scripts and Nimbus can sync against them.

## Notes

- `list` always reflects the true current state of a vault's origin — it's never served from cache, so `ls` is always fresh.
- Content (file bytes) is streamed, not buffered — large files don't get fully loaded into memory during transfer.
- `mv` and `push`/`pull`'s local↔remote transfers only proceed with the destructive step (delete, in `mv`'s case) after the copy has succeeded.
