# nimbus-cli

An interactive shell for managing **vaults** — commands to navigate, transfer, and sync objects across any configured origin (local filesystem, HTTP API, another vault, or a custom shell command).

A vault is a logical tree of objects (folders and files, conceptually) backed by a pluggable **origin** — the actual backend the data lives in. This CLI is a thin frontend over [`nimbus-core`](https://github.com/PeachGB/nimbus/tree/main/crates/core) (which owns the `App`/session state) and [`nimbus-vault`](https://github.com/PeachGB/nimbus/tree/main/crates/vault) (which implements the vault/origin model and does the actual work).

## How it works

`nimbus-cli` runs either way round: pass a command and it runs that one command and exits, pass nothing and it opens a `rustyline`-backed REPL (tab completion for subcommand names and `cd`'s argument).

```bash
nimbus-cli ls              # one command, then exit
nimbus-cli cd docs         # the session is saved, so the next invocation starts here
nimbus-cli --help          # clap's help, as usual
nimbus-cli                 # no arguments: the REPL
```

Both modes drive the same `App` and the same session file, so you can mix them freely. A one-shot command that fails exits `1` with `Error: …` on stderr; a mistyped one gets clap's usage message and its exit code, rather than silently opening the REPL. The REPL's prompt carries your position — `nimbus />>` at the root, `nimbus my-vault/docs>>` inside a vault; the examples below write it as `nimbus>` for brevity.

There's always a special **local vault** (named `LOCAL`) representing your own filesystem — every `put`/`get` moves data between that local vault and whichever remote vault you're working with. You never touch the OS filesystem directly; `put`/`get`'s local-side paths are checked to stay under the configured local root.

## Installation

```bash
cargo install nimbus-cli          # from crates.io
cargo install --path crates/cli   # or from a checkout of the workspace
```

## Configuration

### CLI settings (`~/.config/.nimbus/cli_config.toml`)

```toml
default_local_vault = true       # auto-register the local filesystem as a vault
local_vault_path = "/home/you"   # optional; defaults to $HOME
```

Set `default_local_vault = false` if you don't want the CLI touching your filesystem at all — `put`/`get` will then require every source/destination to be an explicit remote vault (`LOCAL` won't be registered).

### Vault definitions

Each vault you register (other than the automatic `LOCAL` one) is defined by its own `.toml` file — see [`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md) for the full `origin_config` shape (`fs`, `http`, `command`, `vault`), including [`[origin_config.auth]`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#authenticating-an-http-origin) for an `http` origin that needs credentials (a [`nimbus-daemon`](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/README.md), typically — keep the token in an environment variable, not in the config file). Register one with:

```
nimbus> new /path/to/vault.toml
```

or launch the interactive wizard (built on [`nimbus-creator`](https://github.com/PeachGB/nimbus/tree/main/crates/creator)) by running `new` with no path — it prompts for a name, root id, origin type, and that origin's fields, then writes and registers the resulting `vault.toml`. The wizard defaults to saving under `~/.config/.nimbus/vaults/<name>.toml`, so vaults you create stay together and can be found again; the path is editable, and it refuses to overwrite an existing config.

Registering a name that's already taken by a *different* config is refused, since replacing it would leave the original vault unreachable. Use `forget <vault>` to free the name first. Re-registering the **same** config path is fine — that's how an edit to a config file gets picked up.

## Session model

Three things are persisted to `~/.local/state/nimbus/session.toml`: the registry of vaults (name → config path), the vault you had selected, and the directory you were in. They're written on `new`, on `forget`, and on the way out — whether you typed `exit`, hit `Ctrl-C`/`Ctrl-D`, or ran a single command.

That's what makes the one-shot mode usable rather than a novelty:

```
$ nimbus-cli cd my-vault/docs
$ nimbus-cli ls                  # lists my-vault/docs, not the vault list
$ nimbus-cli cd                  # no argument: back to the root, deselecting the vault
```

Restoring the directory has to reach the origin (resolving a path to an `ObjectId` means walking it, one `list` per component), so it happens in an explicit `App::restore_session()` after `App::init()` rather than inside it. It degrades instead of failing: a vault that's gone drops you at the root, and a directory that's gone drops you at that vault's root, each with a warning on stderr.

One consequence worth knowing: since you start off *inside* a vault, the `cd <vault>/<path>` shorthand only applies at the root. Standing in a vault, `cd docs` is a path in that vault. `cd` with no argument gets you back out.

A vault whose config can't be opened right now — say its `[origin_config.auth]` reads a `token_env` you haven't exported in this shell — is reported, skipped for the session, and **stays registered**. It'll be back as soon as its config builds again; only `forget` unregisters a vault.

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
forget <vault>         # stop tracking a vault; its config file and data are left alone
                       # (`LOCAL` can't be forgotten — turn off default_local_vault instead)
```

### Moving data

```
put <path> [vault] [dest]
get <path> [vault] [dest]
```

`put` uploads something from your local filesystem into a vault. `get` downloads something from a vault into your local filesystem. `vault` defaults to whichever vault is currently selected. Arguments are positional.

`dest` defaults to your current position in the vault for `put`, and to the **local vault's root** for `get` — not the shell's working directory, which is outside the local root nearly all the time and so would just fail.

### Operating within a vault

```
mkdir <path> [vault]                 # create a directory
touch <path> [vault]                 # create an empty file
rename <path> <new-name> [vault]     # rename in place; takes a name, not a path
cp <path> <destination> [vault]
mv <path> <destination> [vault]
delete <path> [vault] [-f | --force]
```

`delete` refuses to remove a non-empty directory unless `--force` is given. `mkdir`/`touch` refuse to clobber anything already at the path — `touch` is therefore *not* the usual "create or bump the timestamp", since an origin's `put` truncates.

`rename` takes a bare name, not a path; use `mv` to relocate. It is implemented as a copy under the new name followed by deleting the original, because no origin has a rename primitive — correct everywhere, but it costs a full data copy, which is worth knowing for a large object on a remote origin.

### Paths

Every path argument above is a **`vault:path` spec**:

| Spec | Means |
|------|-------|
| `notes.txt` | relative to the current directory |
| `/docs/notes.txt` | absolute within the current vault |
| `backup:/inbox` | the vault named `backup`, from its root |

A `vault:` prefix is what distinguishes a vault from a directory of the same name — `docs` is always a path, `docs:` is always the vault. The prefix is only recognised when it names a registered vault, so an object whose name contains a colon still resolves as a path.

That prefix is also how `cp`/`mv` **cross vaults**, including between different origin types:

```
nimbus> cp notes.txt backup:/inbox           # into a directory, keeping the name
nimbus> cp notes.txt backup:/inbox/copy.txt  # under a new name
nimbus> mv docs archive:/2026                # directories move recursively
```

The destination may be an existing directory (the object keeps its name) or a path that doesn't exist yet (it lands under that new name). An existing *file* as the destination is refused rather than overwritten.

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

If none of the built-in origins (`fs`, `http`, `vault`) fit your backend, `command` lets you wire up arbitrary shell commands. Each operation gets its own command template with `{id}`, `{name}`, `{size}`, `{content_type}`, `{modified}`, `{kind}` (`leaf`/`branch`) and `{destination}` (where applicable) substituted in, plus any custom `extras` you define — see [`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md) for the full reference:

- `list_cmd` / `get_cmd` must print JSON on stdout matching the object schema — serde's externally-tagged enum, i.e. `{"Leaf": {"name": ..., "id": ..., "meta": {...}}}` or `{"Branch": {..., "children": null}}`.
- `fetch_cmd` streams raw content bytes to stdout.
- `send_cmd` reads the payload from stdin.
- `put_cmd`/`delete_cmd` just need to succeed (exit code 0); stderr is captured for error reporting on failure.

## Notes

- `ls` always reflects the true current state of a vault's origin — it's never served from cache.
- Content (file bytes) is streamed, not buffered — large files don't get fully loaded into memory during transfer.
- `mv` and `push`/`pull`'s local↔remote transfers only proceed with the destructive step (delete, in `mv`'s case) after the copy has succeeded.
- Tab completion currently covers subcommand names and `cd`'s first argument only; other commands' path arguments aren't completed yet. It's a REPL feature — in one-shot mode your shell's completion is what's running.
- To script several commands in one process, pipe them into the REPL (`printf 'select v\nls\n' | nimbus-cli`). One command per invocation is the other option, and the session file is what carries state between them.

## Testing against other origins

[`test/`](https://github.com/PeachGB/nimbus/blob/main/crates/cli/test/README.md) holds a vault config per origin type (`fs`, `http`, `command`, and a vault wrapping another vault), including reference `http` and `command` origin implementations to run them against. Export a scratch `XDG_STATE_HOME` before you start and your real vault registry is left untouched — see that README for the per-origin walkthrough.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/cli/LICENSE-APACHE) or
[MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/cli/LICENSE-MIT) at your option — the same terms as the rest of the workspace.
