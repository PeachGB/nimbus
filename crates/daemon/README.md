# nimbus-daemon

**Status: stub.** `src/main.rs` is currently just `println!("Hello, world!")` — no background-sync logic exists yet. `Cargo.toml` already depends on [`nimbus-vault`](../vault), `tokio`, `toml`, and `tracing`/`tracing-subscriber`, sketching the intended shape: a long-running process that periodically `pull`s/`push`es registered vaults without a user driving [`nimbus-cli`](../cli) or [`nimbus-tui`](../tui) by hand.

The registry it would read is [`nimbus-core`](../core)'s session file (`<state>/nimbus/session.toml`), and the sync itself already exists as `Vault::pull`/`Vault::push` — see [`crates/vault/README.md`](../vault/README.md#syncing-between-origins). What's missing is the scheduling, the config for it, and a decision about how it coordinates with an interactive frontend touching the same vaults.
