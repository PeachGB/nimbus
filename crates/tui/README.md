# nimbus-tui

A ranger-style terminal file manager over nimbus vaults: a list of registered vaults, and inside
each one a browsable object tree, backed by any [`nimbus-vault`](../vault) origin — a local
directory, an HTTP API, a shell command, or another vault. Every operation goes through
[`nimbus-core`](../core)'s `App`, so the TUI and [`nimbus-cli`](../cli) do the same things by the
same code path.

```sh
cargo run -p nimbus-tui
```

![The vault list: every registered vault, with the root-level keybinding hints along the bottom.](../../docs/screenshots/vault-list.png)

Browsing a vault shows its objects with size and modified-time columns, directories grouped
first and accented so they read at a glance:

![Browsing a vault: directories first, then files, each with a size and modified time.](../../docs/screenshots/object-browser.png)

## Keys

### Navigation

| Key | Does |
|-----|------|
| `↑`/`↓` or `k`/`j` | move the selection |
| `→`/`Enter` or `l` | enter a vault or directory, or open a file |
| `←`/`Esc` or `h` | go up a directory, or back to the vault list |
| `q` / `Ctrl-C` | quit |

### Selecting

| Key | Does |
|-----|------|
| `Space` | mark/unmark the object, and step down |
| `/` | filter the listing as you type |
| `Esc` | clear the filter (then: go up a directory) |

Copy, cut and delete act on **every marked object at once**, or on the object under the cursor
when nothing is marked. Marks and filters are dropped when you leave the directory.

### Copy & move

| Key | Does |
|-----|------|
| `y` | yank (copy) the marked objects, or the cursor |
| `d` | cut the marked objects, or the cursor |
| `p` | paste into the directory you're browsing |

The clipboard holds fully-qualified `vault:/path` specs, so navigating to a **different vault**
before pasting is what makes a cross-vault copy or move. Directories are copied recursively.

### Create, rename & delete

| Key | Does |
|-----|------|
| `a` | add a directory here |
| `t` | add an empty file here |
| `r` | rename the selected object |
| `x` / `Del` | delete marked objects, or the cursor — asks first |

`r` opens a prompt pre-filled with the object's **current** name, so a rename is an edit rather
than a retype — changing an extension or adding a suffix costs a few keys. `Enter` applies it,
`Esc` abandons it, and submitting the name unchanged is a silent no-op rather than an error.

`a` and `t` pre-fill the `:` command line instead: a new object's name has to be typed from
scratch either way, so there'd be nothing for a dedicated prompt to offer.

Deleting a directory takes everything inside it. There is **no trash and no undo** — the `y`/`N`
confirmation is the only thing between you and a gone directory.

### View

| Key | Does |
|-----|------|
| `s` / `S` | cycle the sort key (name → size → modified) / reverse the order |
| `.` | show or hide dot-prefixed names |
| `R` | reload the listing from the origin |

Directories always sort before files, whichever key and direction you pick — `S` reverses within
each group rather than burying every directory at the bottom.

### Other

| Key | Does |
|-----|------|
| `n` | create a vault with the setup wizard |
| `x` (on the vault list) | stop tracking a vault |
| `:` | open the command line |
| `?` | toggle the help overlay |

Forgetting a vault only unregisters it — its config file and everything in its origin are left
where they are.

## The `:` command line

`:` accepts the same commands as `nimbus-cli`, with the same syntax:

```
:mkdir docs
:rename notes.txt journal.md
:cp notes.txt backup:/inbox
:delete old.txt --force
:push backup
```

The grammar *is* `AppEvent` (in `event.rs`), which derives `clap::Subcommand` — so the command
line, the keybindings, and the help overlay's COMMANDS section all come from one definition and
can't drift apart. `:help` (or `?`) opens the overlay; it's intercepted before clap, which would
otherwise render help as a parse *error* whose multi-line text is unreadable in a one-line status
bar.

## Paths

| Spec | Means |
|------|-------|
| `notes.txt` | relative to the directory you're in |
| `/docs/notes.txt` | absolute within the current vault |
| `backup:/inbox` | the vault named `backup`, from its root |

A `vault:` prefix is what tells a vault apart from a directory of the same name — `docs` is
always a path, `docs:` is always the vault. A prefix is only recognised when it names a
registered vault, so an object whose name happens to contain a colon still resolves as a path.

`cp`/`mv` take either an existing directory (the object keeps its name) or a path that doesn't
exist yet (it lands under that new name). An existing *file* as the destination is refused rather
than overwritten.

## Opening files

`Enter` on a file fetches it to a temp copy and opens it with the OS default handler
(`xdg-open`/`open`/`start`), falling back to `$EDITOR` (then `$VISUAL`, then `vi`/`notepad`) when
there's no working association. On exit, any edit is **written back** to the object's origin.

Neither path is guaranteed to detach from the terminal — a `.desktop` entry can carry
`Terminal=true` (nvim's ships with it), and without a terminal-emulator wrapper `xdg-open` execs
it straight onto the controlling tty. So both get the same treatment: the event thread is
suspended and raw mode/alt screen released before spawning, then reclaimed once the child exits.
Without that, the TUI and the editor fight over the same stdin and the terminal appears to hang.

Write-back compares content rather than uploading unconditionally, because a plain "look at this
file" would otherwise rewrite it with a new modification time — enough to make the next
`push`/`pull` think it diverged. Three honest outcomes:

- `saved X` — the bytes changed and were written back.
- `closed X — unchanged` — the editor exited and nothing changed.
- `opened X — edits made after this won't be saved back` — the OS opener handed off to a GUI app
  and returned immediately, so there is genuinely nothing left to capture.

## What's here

- **`app.rs`** — `App`: the state machine. Holds the `nimbus-core::App`, the vault/object
  listings, the clipboard, marks, filter, sort, and any pending confirmation or embedded wizard.
  `visible: Vec<usize>` indexes into `objects` and is what the list selects against, so the
  filter and sort work without disturbing what the vault actually reported; anything reading the
  selection must go through `selected_object()`.
- **`event.rs`** — `AppEvent` (the command grammar, see above) and `EventHandler`: the
  tick + crossterm event thread, with `suspend`/`resume` for handing the terminal to a child.
- **`command.rs`** — parses a typed `:` line into an `AppEvent` via clap, and generates the help
  overlay's command list from the same definitions.
- **`ui.rs`** — header, footer and help overlay. The footer is built by `footer_lines()` and the
  layout asks it for its height first, so a long error wraps across up to four lines instead of
  being truncated at the terminal edge.
- **`ui/widgets/`** — the vault and object lists, including the size/modified columns, mark
  indicators, and middle-eliding name truncation (which keeps a file's extension visible).
- **`opener.rs`** — the OS-default opener and `$EDITOR` fallback.

The vault creation wizard ([`nimbus-creator`](../creator)) is driven from this crate's own event
loop rather than its blocking `run()` entry point, so the two never compete for terminal input.

## Marks are names, not indices

`App::marked` is a `HashSet<String>` of object *names*. A refresh renumbers the list but doesn't
rename its contents, so indices would silently come to mean different objects after any mutation.
`refresh_objects` also prunes marks for objects that vanished, or a deleted-then-recreated name
would quietly rejoin the next bulk operation.

## Known limits

- **Every operation blocks the event loop.** Transfers are awaited inline, so the UI freezes for
  the duration with no progress and no cancel. Imperceptible against a local directory; against a
  slow HTTP origin a large transfer looks like a hang.
- `fetch_object_bytes` buffers a whole object in memory rather than using `Origin::fetch`'s
  stream, so opening a large file is proportionally expensive.
- No undo and no trash.

## Commands

```bash
cargo check -p nimbus-tui
cargo test -p nimbus-tui
cargo clippy -p nimbus-tui -- -D warnings
cargo fmt -p nimbus-tui
```

To drive it against origins other than a plain filesystem, see
[`dev/testenv/`](../../dev/testenv/README.md), which builds a sandbox with one vault per origin
type.

## License

Copyright (c) PeachGB <arianmateos@gmail.com>

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
