# nimbus-creator

An interactive Ratatui wizard that builds a [`nimbus-vault`](../vault) `vault.toml`. Embeddable from another app that's already driving its own terminal ([`nimbus-cli`](../cli)'s `new` with no path and [`nimbus-tui`](../tui)'s `n` key both run it in-process), or runnable standalone via `nimbus-creator`'s own binary.

## How it works

`run(terminal)` (in `lib.rs`) takes an already-initialized `Terminal` — the caller owns terminal setup/teardown (`ratatui::init`/`restore` or equivalent) — and drives a step machine (`app.rs::Step`): `Name` → `RootId` → `SelectOrigin` → one `Field(i)` per field the chosen origin needs → `SavePath` → `Confirm`. On confirm, it builds the matching `VaultConfig`/`OriginConfig` and writes it to the chosen path.

Progress is linear and `Esc` at any point cancels the whole wizard, with one exception: a **refused save returns from `Confirm` to `SavePath`**, so a bad path can be corrected without losing everything already typed.

Returns `Some(path)` if the wizard completed (the path the config was written to), or `None` if the user cancelled.

## Where configs are saved

The save-path step is pre-filled with `VaultConfig::default_path(name)` — `<config>/.nimbus/vaults/<name>.toml`, via [`nimbus-vault`](../vault)'s `config_home()`. It's still editable, so an explicit path works as before; the default just stops configs from landing in whatever directory the program happened to be launched from, where they'd be hard to find again.

Saving **refuses to overwrite an existing file**. `VaultConfig::save` writes through, and because the default path is derived from the vault name, reusing a name is exactly how you'd land on an occupied one — so accepting the default twice would otherwise destroy the first vault's config. The wizard reports the clash and returns to the save-path step.

Note that the file is written *before* the caller registers it, so a name that collides with a vault whose config lives somewhere else will write the config and then fail to register (`nimbus-core`'s `new_vault` refuses to displace an existing name). This crate has no dependency on `nimbus-core` and so can't check the registry; the stray file is harmless and the error says what happened.

## What's here

- **`app.rs`** — `App`: the wizard's state machine. `App::run` owns a blocking terminal event loop; `App::handle_key_event` is exposed separately, along with `is_running()`/`into_outcome()`, so tests — or a caller with its own event loop, which is how `nimbus-tui` embeds it — can drive the wizard without a live terminal or a second event-reading thread. Text-entry steps support `Tab`-based path completion (`path_suggestions`) for fields marked `path_completable` (currently just `fs`'s `root`), including `~`-expansion.
- **`builder.rs`** — `OriginKind` (`Fs` / `Http` / `Command` / `Vault`, mirroring `OriginConfig`'s variants) and `FieldSpec`, describing the prompts/keys/optionality the wizard needs to collect per origin kind. `OriginKind::build` turns collected field values into the matching `OriginConfig`.
- **`event.rs`** — the Ratatui event-driven-template event loop/handler (tick + crossterm + app events).
- **`ui.rs`** — renders the current step.
- **`src/bin/creator.rs`** — standalone binary entry point (`ratatui::init`/`run`/`restore`), for running the wizard on its own outside `nimbus-cli`.

## Origin fields collected per kind

- `fs` — `root` (path-completable).
- `http` — `base_url` (optional), `list_url`, `fetch_url`, `get_url`, `put_url`, `send_url`, `delete_url`.
- `command` — `list_cmd`, `fetch_cmd`, `get_cmd`, `put_cmd`, `send_cmd`, `delete_cmd`, `extras` (optional, `k=v,k2=v2` syntax).
- `vault` — `path` (inner vault config path).

## Commands

```bash
cargo check -p nimbus-creator
cargo test -p nimbus-creator
cargo clippy -p nimbus-creator -- -D warnings
cargo fmt -p nimbus-creator
cargo run -p nimbus-creator --bin creator   # run the wizard standalone
```
