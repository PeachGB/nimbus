# nimbus-core

Session/vault-management logic shared by nimbus's frontends (currently [`nimbus-cli`](../cli); [`nimbus-tui`](../tui) is still an unimplemented stub). Owns the `App` — registered vaults, the current vault/working directory, and the local staging vault — plus the on-disk config `App` is built from. Frontends drive it through `App`'s methods and are responsible for their own input/output loop; `nimbus-core` has no terminal/UI code of its own.

## What's here

- **`config.rs`** — `CliConfig`: the on-disk shape read from `~/.config/.nimbus/cli_config.toml` (`default_local_vault`, defaulting to `true`; `local_vault_path`, defaulting to `$HOME`). `CliConfig::load()` returns the default config if the file doesn't exist yet, rather than erroring.
- **`app.rs`** — `App`: holds every registered `Vault` (by name), the special `LOCAL` vault (the user's own filesystem, when `default_local_vault` is enabled), the current vault/cwd, and vault-config paths so they can be re-registered on the next run. `App::init()` loads `CliConfig`, restores previously-registered vaults from `~/.local/state/nimbus/session.toml`, and (re-)registers `LOCAL` if configured. `App::save()` writes the registered vault-config paths back to that session file.

## Commands exposed by `App`

`ls`, `vaults`, `select`, `new_vault`, `cd` (plus `cd_completions`, used by `nimbus-cli`'s tab completion), `put`, `get`, `cp`, `mv`, `delete`, `push`, `pull`, `exit`. See [`crates/cli/README.md`](../cli/README.md) for the user-facing command reference these map to.

- `put`/`get`/`cp`/`mv` all follow the same pattern: resolve the source path to an `ObjectId` via `Vault::find`, `get` its `Object`, `put` it under the resolved destination, and — for `Leaf`s only — `fetch` the payload from the source and `send` it to whatever `Object` `put` returned (not the pre-`put` object; see [`crates/vault/README.md`](../vault/README.md#the-put-contract)).
- `put`/`get` additionally resolve local-filesystem paths through `resolve_local_path`, which canonicalizes the input and rejects anything outside the configured local root — this is the boundary that keeps `LOCAL` from touching files outside `local_vault_path`.
- `cd` with no vault selected treats the path's first component as a vault name (`select`s it) and recurses on the remainder; with a vault selected, it resolves the path relative to the current directory via `Vault::find`.

## Commands

```bash
cargo check -p nimbus-core
cargo test -p nimbus-core
cargo clippy -p nimbus-core -- -D warnings
cargo fmt -p nimbus-core
```
