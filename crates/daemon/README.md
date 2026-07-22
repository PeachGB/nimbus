# nimbus-daemon

**Status: stub.** `src/main.rs` is currently just `println!("Hello, world!")` — no background-sync logic exists yet. `Cargo.toml` already depends on [`nimbus-vault`](../vault), `tokio`, `toml`, and `tracing`/`tracing-subscriber`, sketching the intended shape: a long-running process that periodically `pull`s/`push`es registered vaults without a user driving `nimbus-cli` by hand.
