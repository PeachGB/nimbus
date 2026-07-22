# nimbus-creator

An interactive Ratatui wizard that builds a [`nimbus-vault`](../vault) `vault.toml`. Embeddable from another app that's already driving its own terminal (`nimbus-cli`'s `new` with no path runs it in-process), or runnable standalone via `nimbus-creator`'s own binary.

## How it works

`run(terminal)` (in `lib.rs`) takes an already-initialized `Terminal` — the caller owns terminal setup/teardown (`ratatui::init`/`restore` or equivalent) — and drives a linear step machine (`app.rs::Step`): `Name` → `RootId` → `SelectOrigin` → one `Field(i)` per field the chosen origin needs → `SavePath` → `Confirm`. There's no backward navigation between steps; `Esc` at any point cancels the whole wizard. On confirm, it builds the matching `VaultConfig`/`OriginConfig` and writes it to the chosen path.

Returns `Some(path)` if the wizard completed (the path the config was written to), or `None` if the user cancelled.

## What's here

- **`app.rs`** — `App`: the wizard's state machine. `App::run` owns a blocking terminal event loop; `App::handle_key_event` is exposed separately so tests (or a caller with its own event loop) can drive the wizard without a live terminal. Text-entry steps support `Tab`-based path completion (`path_suggestions`) for fields marked `path_completable` (currently just `fs`'s `root`), including `~`-expansion.
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
