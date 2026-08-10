# nimbus-core

Session/vault-management logic shared by nimbus's frontends ([`nimbus-cli`](https://github.com/PeachGB/nimbus/tree/main/crates/cli) and
[`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui)). Owns the `App` — registered vaults, the current vault/working directory,
and the local staging vault — plus the on-disk config `App` is built from. Frontends drive it
through `App`'s methods and are responsible for their own input/output loop; `nimbus-core` has no
terminal/UI code of its own.

```bash
cargo add nimbus-core
```

## Where this sits

[`nimbus-vault`](https://github.com/PeachGB/nimbus/tree/main/crates/vault) is the foundation — the vault/origin model, the only code that reaches a
backend, and what everything else in the workspace is built on. This crate is the layer above
it: the reusable core of the *applications* built on that model, meaning everything a nimbus
frontend does that isn't drawing to a screen.

Concretely, that's a registry of named vaults, which one is selected and where you are inside
it, the `vault:path` spec that addresses any object in any of them, one implementation of each
command (`cd`, `ls`, `cp`, `mv`, `put`, `get`, `mkdir`, `rename`, `delete`, `push`, `pull`), and
the session file carrying all of it between runs.

[`nimbus-cli`](https://github.com/PeachGB/nimbus/tree/main/crates/cli) and [`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui) are both shells around this, which is why a `cp` means
the same thing in each — there's one implementation, not two that drifted. `App` accommodates
both by exposing two overlapping surfaces: the command methods above, mirroring what a user
types, and data-returning ones (`vault_names`, `list_cwd`, `fetch_object_bytes`,
`write_object_bytes`) for a frontend that renders rather than prints. A third frontend — a GUI,
a web UI, an editor plugin — starts here for the same reason.

## Build a frontend on it

If that's what you're here for: please do, it's the reason this crate is split out at all.

A frontend owes `App` two things — an input loop, and something that displays a `Vec<Object>`.
Everything under that line already exists and is shared: resolving a `vault:path` to an object,
streaming bytes between two unrelated origins, the local-vault boundary, the vault registry,
session persistence. Nothing here assumes a terminal or pulls in one, so a GUI or an HTTP
service is as ordinary a consumer as another TUI is.

[`nimbus-cli`](https://github.com/PeachGB/nimbus/tree/main/crates/cli) is the smaller of the two existing frontends and the better one to read first —
it's close to a direct mapping from a parsed command to an `App` method. [`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui) shows
the rendering side, including how to drive an embedded [`nimbus-creator`](https://github.com/PeachGB/nimbus/tree/main/crates/creator) wizard from your
own event loop.

Issues and pull requests are welcome, and an `App` method that's awkward to drive from outside
a terminal is a bug worth reporting — the whole point of the split is that it shouldn't be.

## What's here

- **`config.rs`** — `CliConfig`: the on-disk shape read from `<config>/.nimbus/cli_config.toml`
  (`default_local_vault`, defaulting to `true`; `local_vault_path`, defaulting to `$HOME`). The
  directory comes from `nimbus_vault::config_home()` rather than being rebuilt here, so the CLI
  config and the vault configs written by [`nimbus-creator`](https://github.com/PeachGB/nimbus/tree/main/crates/creator) can't drift apart.
  `CliConfig::load()` returns the default config if the file doesn't exist yet, rather than
  erroring.
- **`app.rs`** — `App`: holds every registered `Vault` (by name), the special `LOCAL` vault (the
  user's own filesystem, when `default_local_vault` is enabled), the current vault/cwd, and
  vault-config paths so they can be re-registered on the next run. `App::init()` loads
  `CliConfig`, opens the vaults registered in `<state>/nimbus/session.toml`, and (re-)registers
  `LOCAL` if configured. `App::save()` writes that session file back.

  The session path is a **field** (`state_path`), not a constant, so tests can point it at a
  scratch file. That isn't stylistic: `save()` used to write the real session file
  unconditionally, so `cargo test` rewrote the developer's own vault registry.

## The session file

`SavedState` holds three things: `vault_configs` (the `name → config path` registry),
`current_vault`, and `cwd_path`. The last two are what let a one-shot `nimbus-cli ls` pick up
where the previous `nimbus-cli cd docs` left off. Both are `#[serde(default)]`, so session files
written before they existed still load.

Restoring the directory can't happen in `init()`: turning a path back into an `ObjectId` means
walking it with one `Vault::find` per component, which is `async` and reaches the origin, and
`init()` is a sync constructor. So `init()` parks what it read in a private field and
**`App::restore_session()`** (async) applies it. Frontends that want the behaviour call it right
after `init()`; ones that don't (the TUI opens on its own vault picker) simply don't.

Two rules protect the session from being destroyed by a transient failure:

- A vault whose config **doesn't build right now** is warned about and skipped for the session,
  but stays in the registry. Config building can fail for reasons that aren't permanent — an
  `[origin_config.auth]` whose `token_env` isn't exported in this shell — and dropping the entry
  would have the next `save()` unregister the vault for good. `open_vaults` therefore borrows
  the registry instead of consuming it, and `forget_vault` checks the *registry*, so a vault that
  can't be opened can still be unregistered.
- `save()` leaves a session that was read but never restored (a frontend that skips
  `restore_session`) exactly as it found it, rather than overwriting it with the root. Otherwise
  opening the TUI would wipe the CLI's position.

`restore_session` degrades rather than failing: a vault that's gone leaves you at the app root, a
directory that's gone leaves you at that vault's root (`select` has already put you there), each
with a warning on stderr.

## Commands exposed by `App`

`ls`, `vaults`, `select`, `new_vault`, `forget_vault`, `cd` (plus `cd_completions`, used by
`nimbus-cli`'s tab completion), `put`, `get`, `mkdir`, `touch`, `rename`, `cp`, `mv`, `delete`,
`push`, `pull`, `exit`. See [`crates/cli/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/cli/README.md) for the user-facing
command reference these map to. `restore_session` sits alongside them but isn't a user command —
it's the async half of `init`, described above.

`exit` is the odd one out: it `save()`s and then calls `std::process::exit(0)` itself, rather
than reporting back that the frontend should shut down. `nimbus-tui` therefore never routes its
own quit through it — it has an event loop and a terminal to restore first.

Alongside those are data-returning methods, for frontends that render rather than print:
`vault_names()`, `list_cwd()`, `fetch_object_bytes(id)` and `write_object_bytes(id, bytes)`. The
last two are what let the TUI open a file in an editor and save the result back.

- `put`/`get`/`cp`/`mv` all follow the same pattern: resolve the source path to an `ObjectId` via
  `Vault::find`, `get` its `Object`, `put` it under the resolved destination, and — for `Leaf`s
  only — `fetch` the payload from the source and `send` it to whatever `Object` `put` returned
  (not the pre-`put` object; see
  [`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#the-put-contract)).
- `put`/`get` additionally resolve local-filesystem paths through `resolve_local_path`, which
  canonicalizes the input and rejects anything outside the configured local root — this is the
  boundary that keeps `LOCAL` from touching files outside `local_vault_path`. `get` with **no**
  destination targets the local vault's root, not the process's working directory, which would
  otherwise fail anywhere except inside the local root.
- `cd` with no vault selected treats the path's first component as a vault name (`select`s it)
  and recurses on the remainder; with a vault selected, it resolves the path relative to the
  current directory via `Vault::find`.

## Path specs

`resolve_spec` is how nearly every command turns a typed argument into a `(vault, absolute path)`
pair:

| Spec | Resolves to |
|------|-------------|
| `notes.txt` | relative to `cwd_path`, in the current (or explicitly named) vault |
| `/docs/notes.txt` | absolute within that vault |
| `backup:/inbox` | the vault named `backup`, from **its** root |

`split_vault_spec` only treats a `name:` prefix as a vault when `name` is actually registered, so
an object whose name contains a colon still resolves as a path. A qualified spec resolves from
that vault's root because there is no meaningful "current directory" in a vault you aren't
standing in.

## Recursion, renaming, and the guards

- **`deep_copy`** is what makes `cp`/`mv`/`rename` work on directories. `Vault::put` on a
  `Branch` only creates the entry itself — it knows nothing about children — so without recursion
  a moved directory would arrive empty and the source would then be deleted. It takes an optional
  `rename_to`, which applies to the **top level only**; descendants keep their own names.
- **`rename`** is a copy under the new name followed by deleting the original, because `Origin`
  has no rename primitive (`put` always writes an object under its own `get_name()`). Correct for
  every origin, but it costs a **full data copy** — worth knowing for a large object on a remote
  origin. A real `Origin::rename` with an `fs::rename` fast path is the fix if that ever matters.
- **`cp`/`mv` destinations** may be an existing directory (the object keeps its name) or a path
  that doesn't exist yet (it lands in its parent under that new name). An existing *file* is
  refused rather than overwritten, because `put` truncates.
- The **into-itself guard** rejects any destination whose path starts with the source's, with no
  exemption for the two being equal — pasting a directory into itself resolves to exactly that,
  and `deep_copy` would otherwise recurse into the copy it had just made, forever. `starts_with`
  is component-wise, so `cp a.txt a.txt.bak` and `cp docs docs2` are unaffected.
- **`delete`** refuses a non-empty directory unless forced. The check only runs for directories:
  asking an origin to list a leaf is an error, not an empty listing, so applying it universally
  made every file undeletable without `--force`.
- **`new_vault`** refuses a name already registered to a *different* config path — silently
  replacing the entry would leave the original vault unreachable, with nothing pointing at its
  data. Re-registering the **same** path is allowed, and is how an edit to a config file gets
  picked up (including fixing one that failed to open). `forget_vault` frees a name; it is
  registry bookkeeping only and never touches the vault's config file or the data in its origin.

## Commands

```bash
cargo check -p nimbus-core
cargo test -p nimbus-core
cargo clippy -p nimbus-core -- -D warnings
cargo fmt -p nimbus-core
```

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/core/LICENSE-APACHE) or
[MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/core/LICENSE-MIT) at your option — the same terms as the rest of the workspace.
