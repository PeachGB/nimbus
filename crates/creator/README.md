# nimbus-creator

An interactive Ratatui wizard that builds a [`nimbus-vault`](https://github.com/PeachGB/nimbus/tree/main/crates/vault) `vault.toml`. Embeddable from another app that's already driving its own terminal ([`nimbus-cli`](https://github.com/PeachGB/nimbus/tree/main/crates/cli)'s `new` with no path and [`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui)'s `n` key both run it in-process), or runnable standalone via its own binary.

Reaching for it directly is the unusual case: [`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui)'s `n` key runs this wizard and registers what it writes, which is the whole job in one step.

## Installing it standalone

```bash
cargo install nimbus-creator          # from crates.io
cargo install --path crates/creator   # or from a checkout of the workspace
cargo add nimbus-creator              # or embed it in your own Ratatui app
```

Standalone, the binary writes a `vault.toml` and stops there — registering it is a frontend's job (`new <path>` in [`nimbus-cli`](https://github.com/PeachGB/nimbus/tree/main/crates/cli)).

## How it works

`run(terminal)` (in `lib.rs`) takes an already-initialized `Terminal` — the caller owns terminal setup/teardown (`ratatui::init`/`restore` or equivalent) — and drives a step machine (`app.rs::Step`): `Name` → `RootId` → `SelectOrigin` → one `Field(i)` per field the chosen origin needs → `Encrypt` (`y`/`n`) → `KeyPath` (only after `y`) → `SavePath` → `Confirm` → `KeyBackup` (only when encrypting). On confirm, it builds the matching `VaultConfig`/`OriginConfig` and writes it to the chosen path; an encrypted vault first goes through the key-backup screen (see [Encrypted vaults](#encrypted-vaults)).

Progress is linear and `Esc` at any point cancels the whole wizard, with two exceptions: a **refused or failed save returns to `SavePath`**, and a **key path that is already taken returns to `KeyPath`**, so a bad path can be corrected without losing everything already typed.

Returns `Some(path)` if the wizard completed (the path the config was written to), or `None` if the user cancelled. `run_for_outcome(terminal)` returns an `Outcome` instead: `config` (the same `Option<PathBuf>`) and `unsaved_key_file`, a key file the wizard wrote for a config it then didn't save (see [Encrypted vaults](#encrypted-vaults)). Embedders driving `App::handle_key_event` themselves get the same from `App::into_result()` or `App::unsaved_key_file()`.

Errors show in a footer that wraps (up to four lines), with the essential part first, so a long path doesn't cut them off.

## Where configs are saved

The save-path step is pre-filled with `VaultConfig::default_path(name)` — `<config>/.nimbus/vaults/<name>.toml`, via [`nimbus-vault`](https://github.com/PeachGB/nimbus/tree/main/crates/vault)'s `config_home()`. It's still editable, so an explicit path works as before; the default just stops configs from landing in whatever directory the program happened to be launched from, where they'd be hard to find again.

Saving **refuses to overwrite an existing file**. `VaultConfig::save` writes through, and because the default path is derived from the vault name, reusing a name is exactly how you'd land on an occupied one — so accepting the default twice would otherwise destroy the first vault's config. The wizard reports the clash and returns to the save-path step.

Note that the file is written *before* the caller registers it, so a name that collides with a vault whose config lives somewhere else will write the config and then fail to register (`nimbus-core`'s `new_vault` refuses to displace an existing name). This crate has no dependency on `nimbus-core` and so can't check the registry; the stray file is harmless and the error says what happened.

## Encrypted vaults

After the origin's fields, the wizard asks **encrypt this vault at rest?** (`y`/`n`). It isn't another origin kind: `y` wraps whichever kind you chose in a `crypt` origin (`OriginConfig::Crypt` with that origin as `inner`), so the storage only ever holds ciphertext.

- **Key path.** `y` asks where to create the key file, pre-filled with `<config>/.nimbus/keys/<vault>.key` (via `config_home()`; next to `vaults/` but apart from it, so copying configs around doesn't copy keys). The answer follows the same rule as the other path answers: `~` expands, a relative path is taken against the current directory, and it is saved absolute as the config's `key_file`.
- **Shown once per confirmation, written after.** On confirm the key is generated in memory only, and the next screen shows it: what losing it means, where it *will be* written, and the key itself. It's the same key throughout, so after a key-path collision the same key is shown again. Nothing is on disk yet. Typing `I have a backup` (case doesn't matter) writes the key file, then the config. Any other answer is refused and nothing is written; `Esc` before the phrase writes nothing at all and drops the key.
- **The key file** is written readable by you only (`0600` on Unix; on Windows it takes the directory's ACL) and **never overwrites** an existing file. A key directory that doesn't exist yet is created first, `0700` on Unix for each directory the wizard creates; an existing directory's mode is never changed. If the key path turns out to be taken, the wizard goes back to the key-path step and keeps the *same* key, so the one you backed up is the one that gets written; after a new path and confirm, the same key is shown again.
- **A failed config save** after the key was written goes back to the save path, with an error naming where the key was written. The key file is kept, never deleted, and a retry saves the config without writing the key again; the config always names the file that was actually written. Leaving with `Esc` instead ends with a notice: "a key file was written and kept at <path>; the vault config wasn't saved, so write a config for it by hand (key_file = <path>) or delete the key". `nimbus-cli new` and the standalone binary print it once the terminal is restored; `nimbus-tui` puts it in the status bar.
- **Losing the key loses the data.** Nobody, you included, can decrypt the vault without it; keep the backup somewhere other than the vault's storage.
- The key-backup screen is the only place the key is displayed, inside the wizard's alternate screen. Don't run the wizard under a terminal recorder (script, asciinema, tmux logging): it would capture the key. The key and its text are wiped from memory when dropped, but the copies in ratatui's frame buffers are not zeroized: ratatui owns those buffers.

See [Encrypted vaults in `nimbus-vault`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#encrypted-vaults-type--crypt) for what `crypt` stores and how to recover a file with `nimbus-cli decrypt`.

## What's here

- **`app.rs`** — `App`: the wizard's state machine. `App::run` owns a blocking terminal event loop; `App::handle_key_event` is exposed separately, along with `is_running()`/`into_outcome()`, so tests — or a caller with its own event loop, which is how `nimbus-tui` embeds it — can drive the wizard without a live terminal or a second event-reading thread. Text-entry steps support `Tab`-based path completion (`path_suggestions`) for fields marked `path_completable` (currently just `fs`'s `root`), including `~`-expansion. A typed path answer gets the same `~`/`~/` expansion whether or not `Tab` was pressed, and is then made absolute against the current directory (see `builder.rs` below).
- **`builder.rs`** — `OriginKind` (`Fs` / `Http` / `Command` / `Vault`, mirroring `OriginConfig`'s variants) and `FieldSpec`, describing the prompts/keys/optionality the wizard needs to collect per origin kind. `OriginKind::build` turns collected field values into the matching `OriginConfig`, making path answers (`fs` `root`, `vault` `path`) absolute against the current directory: that's what the prompt completes against, while a saved config's relative paths would resolve against the config's own directory. Fields are flat strings, so a structured value gets assembled here: `extras` parses `k=v,k2=v2` into a map, and `auth_token_env` becomes an `[origin_config.auth]` bearer table (or `HttpAuth::None` when left empty).
- **`event.rs`** — the Ratatui event-driven-template event loop/handler (tick + crossterm + app events).
- **`ui.rs`** — renders the current step. The key-backup screen is the only one that shows a key.
- **`src/bin/creator.rs`** — standalone binary entry point (`ratatui::init`/`run`/`restore`), for running the wizard on its own outside `nimbus-cli`.

## Origin fields collected per kind

Encryption isn't a kind of its own: the `Encrypt` step that follows these fields wraps any of them (see [Encrypted vaults](#encrypted-vaults)).

- `fs` — `root` (path-completable).
- `http` — `base_url` (optional), `list_url`, `fetch_url`, `get_url`, `put_url`, `send_url`, `delete_url`, `auth_token_env` (optional). The last one becomes an `[origin_config.auth]` bearer table reading that environment variable; leaving it empty means the origin sends no credentials. The wizard deliberately offers only the env-var form — a config file is no place for a secret. `token`/`token_file` are for hand-edited configs, see [`nimbus-vault`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#authenticating-an-http-origin). The wizard doesn't ask for `[origin_config.tls]` or `rename_url` either; add those to the config by hand (see [`nimbus-vault`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#tls-origin_configtls)). Nor does it offer `key_env` for an encrypted vault: it always writes a key file, and switching to `key_env` is a hand edit (see [`nimbus-vault`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#encrypted-vaults-type--crypt)).
- `command` — `list_cmd`, `fetch_cmd`, `get_cmd`, `put_cmd`, `send_cmd`, `delete_cmd`, `extras` (optional, `k=v,k2=v2` syntax).
- `vault` — `path` (inner vault config path).

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/creator/LICENSE-APACHE) or
[MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/creator/LICENSE-MIT) at your option — the same terms as the rest of the workspace.
