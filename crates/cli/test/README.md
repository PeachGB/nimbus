# test vaults

One config per `type` of `OriginConfig` (`crates/vault/src/config.rs`: `fs`, `command`,
`http`, `vault` and `crypt`), for trying the REPL by hand against each one. The commands below are typed at the REPL prompt
(`nimbus />>`).

The paths inside the `.toml` files are relative to each config file's own directory
(`root = "data"`, `path = "../fs/fs.toml"`, and `{config_dir}` for the command origin), so
they work whatever directory you start from. The commands below use paths from the repo
root; from anywhere else, give `new` the path to the config from there. `new` stores the
config's absolute path, so a registered vault opens from any directory later.

Important: `nimbus-cli` persists `vault_configs` in
`$XDG_STATE_HOME/nimbus/session.toml` (or `~/.local/state/nimbus/session.toml` if
`XDG_STATE_HOME` isn't set) on Linux; see
[Configuration locations](../../../README.md#configuration-locations) for macOS and Windows. To keep your real session clean while trying this, run
the binary with a temporary `XDG_STATE_HOME`:

```sh
export XDG_STATE_HOME=$(mktemp -d)
cargo run -p nimbus-cli
```

A vault registered before 0.4.0 was stored with the path as typed; if one doesn't open,
run `new <config>` again from the directory you originally registered it from (or
`forget <vault>` first if the file moved or you're elsewhere), or start over with a fresh
`XDG_STATE_HOME=$(mktemp -d)`.

Keep in mind too that the `LOCAL` vault (used by `put`/`get`/`push`/`pull`) points at
your `$HOME` by default. You don't need it to try `ls`/`cd`/`select` on the vaults
below, but if you want to try `put`/`get` without risking real files, put this in
`~/.config/.nimbus/cli_config.toml`:

```toml
default_local_vault = true
local_vault_path = "/path/to/a/sandbox/folder"
```

## fs/ — `type = "fs"`

A vault over a real directory (`fs/data/`, with one file and one subdirectory).
Nothing needs to be running.

```
new crates/cli/test/fs/fs.toml
select fs-vault
ls
cd docs
ls
```

## command/ — `type = "command"`

A vault over `command/data/`, wrapped in a script (`cmd-vault.sh`) that turns real
listings/stats into the JSON `OriginCommand` expects (`list_cmd`/`get_cmd` must print
the same shape as the `Object` enum). The script needs `jq`.

```
new crates/cli/test/command/command.toml
select cmd-vault
ls
```

`[origin_config.extras]` defines extra placeholders, here `{root}`, set to
`"{config_dir}"`, the directory holding `command.toml`, so the scripts are found wherever
you run from. Every command
template can use them, `put_cmd` and `send_cmd` included (see
`crates/vault/README.md#command-templating`). Every command runs under `sh -c`, so a
template can be a command with arguments. `{kind}` (`leaf` or `branch`) tells `put_cmd`
whether it is creating a file or a directory.

`command/send.sh` is an atomic `send_cmd`: it takes the destination as an argument
(`{root}/data/{id}`), writes stdin to a temp file in the same directory, and only `mv`s
it over the destination at the end. If nimbus aborts the transfer, it kills the script
before closing stdin, so the destination is never left half-written.

Placeholder values arrive already quoted for `sh` (see
`crates/vault/README.md#values-are-shell-quoted`): don't wrap `{id}` or `{name}` in
quotes inside the template.

## http/ — `type = "http"`

A vault against a toy HTTP server (`server.py`, stdlib only) that serves `http/data/`
with the same JSON contract `OriginHTTP` expects. Start it first:

```sh
python3 crates/cli/test/http/server.py   # port 8787 by default
```

and in another terminal (or in the same REPL, in parallel):

```
new crates/cli/test/http/http.toml
select http-vault
ls
```

`server.py` implements `list`/`get`/`fetch`/`put`/`send`/`delete` in full, so it's
good for trying the whole `put`/`get`/`delete` cycle without touching the `LOCAL`
vault.

### against `nimbus-daemon`

`server.py` is still useful because it's a third-party HTTP origin (it implements the
contract without sharing code with nimbus), but the repo now ships a real server. To
try the same origin against
[`nimbus-daemon`](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/README.md),
point it at the `fs/` folder itself. `fs.toml` is the only config there, and its
`root = "data"` resolves against its own directory, so it is served as is (the
`--vaults` flag is relative to the current directory, here the repo root):

```sh
cargo run -p nimbus-daemon -- --vaults crates/cli/test/fs --bind 127.0.0.1:8788
```

That serves `fs-vault` (the `name` inside `fs.toml`, not the file name) at
`http://127.0.0.1:8788/v/fs-vault`. The client vault points there with the six default
templates (`/list/{id}` and friends). Add `--token s3cr3t` to the daemon and
`[origin_config.auth]` with `token_env` to the vault to try the authenticated path;
see
[`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#authenticating-an-http-origin).

## vault-of-vault/ — `type = "vault"`

Wraps `fs/fs.toml` as the origin of another vault (`OriginVault`), to check that
nesting vaults works.

```
new crates/cli/test/vault-of-vault/nested.toml
select nested-vault
ls
cd docs
ls
```

## crypt/ — `type = "crypt"`

An encrypted vault over a local `fs` origin (`crypt/data/`). The files in `data/` are
ciphertext (each starts with `NMBC`), plus the hidden `.nimbus-crypt` marker; through the
vault they read as `fs/`'s `notes.txt` and `docs/todo.txt`, copied in with `cp`.

**`TEST-ONLY.key` is public: it is committed to the repository, so anyone can read it and
this vault protects nothing.** It must never protect real data; make your own key with
`keygen`. Git checks it out readable by everyone, so nimbus warns about it until you
`chmod 600` it:

```sh
chmod 600 crates/cli/test/crypt/TEST-ONLY.key
```

```
new crates/cli/test/crypt/crypt.toml
select crypt-vault
ls
cd docs
ls
```

Recovering a file without any vault, as you would from a backup copy of `data/`:

```sh
cargo run -p nimbus-cli -- decrypt crates/cli/test/crypt/data/notes.txt /tmp/notes.txt \
    --key-file crates/cli/test/crypt/TEST-ONLY.key
```

A new key of your own: `cargo run -p nimbus-cli -- keygen /path/to/new.key` (it refuses to
overwrite an existing file).
