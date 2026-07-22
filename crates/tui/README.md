# nimbus-tui

**Status: stub.** This crate is scaffolding — a [Ratatui] app generated from the [event driven template], not yet wired up to [`nimbus-core`](../core)/[`nimbus-vault`](../vault). `app.rs` still has the template's placeholder counter (`Left`/`Right` to decrement/increment, `q`/`Esc`/`Ctrl-C` to quit); there is no vault browsing yet.

The intended shape (see [`crates/core/README.md`](../core/README.md)) is a `nimbus-core::App`-driven terminal UI: navigate registered vaults, `cd`/`ls`/`put`/`get`/etc., the same operations [`nimbus-cli`](../cli) exposes as REPL commands, but rendered as a Ratatui frontend rather than a line-editor prompt. [`nimbus-creator`](../creator)'s wizard is written to be embeddable from exactly this kind of caller, for a `new`-vault flow inside the TUI.

[Ratatui]: https://ratatui.rs
[event driven template]: https://github.com/ratatui/templates/tree/main/event-driven

## License

Copyright (c) PeachGB <arianmateos@gmail.com>

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
