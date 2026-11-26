# nimbus-daemon

Serves a directory of vault configs over HTTP or HTTPS, so another nimbus can mount them through the [`http` origin](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md). It's the other half of `OriginHTTP`: one machine holds the data, the rest talk to it with the same `ls`/`get`/`put`/`push`/`pull` they'd use on a local folder.

It is a *server*, not a scheduler — it doesn't sync anything on its own. Periodic background `pull`/`push` (the original sketch for this crate) remains unimplemented.

A daemon on its own has no client. [`nimbus-tui`](https://github.com/PeachGB/nimbus/tree/main/crates/tui) (`cargo install nimbus-tui`) is the quickest one to point at it: an `http` vault config aimed at `http://host:8080/v/<name>` and the served vault browses like a local directory.

## Running it

```bash
cargo install nimbus-daemon                    # from crates.io
cargo install --path crates/daemon             # or from a checkout of the workspace

nimbus-daemon                                  # config file only
nimbus-daemon --vaults ./vaults --bind 0.0.0.0:8080 --read-only
```

Settings come from three places, each overriding the one below it:

1. command-line flags
2. the config file (`~/.config/.nimbus/daemon_config.toml`, or `--config <file>`)
3. built-in defaults

A `--config` you type has to exist — a path that doesn't is far more likely a typo than a request to create one. The default path is allowed to be missing: the first run writes it, filled in with the built-in defaults and pointed at `~/.config/.nimbus/vaults` (where `nimbus-cli` saves vault configs), creating that directory too so the daemon comes up serving whatever is already there. A config home that can't be written to is only a warning; the defaults still run.

Unknown keys in the file are an error rather than being ignored, so a typo doesn't silently leave you on the default.

### `daemon_config.toml`

```toml
vaults_path = "/srv/nimbus/vaults"   # required (or --vaults)
bind = "127.0.0.1:8080"              # default: loopback
read_only = false                    # refuse every write

[auth]                               # default: no authentication
type = "bearer"
token_file = "/etc/nimbus/token"     # or token = "s3cr3t", inline

[tls]                                # optional: serve HTTPS
cert_file = "/etc/nimbus/cert.pem"   # PEM chain, leaf first
key_file  = "/etc/nimbus/key.pem"
```

| Key | Flag | Default |
|-----|------|---------|
| `vaults_path` | `--vaults <DIR>` | — (required) |
| `bind` | `--bind <ADDR>` | `127.0.0.1:8080` |
| `read_only` | `--read-only` | `false` |
| `[auth]` (`token` or `token_file`) | `--token <TOKEN>` | `type = "none"` |
| `[tls]` | `--tls-cert <FILE>` + `--tls-key <FILE>` (both or neither) | none: plain HTTP |
| `allow_insecure_http` | — (file only) | `false` |

**Relative paths** in the file (`vaults_path`, `[tls]` `cert_file` and `key_file`, the `[auth]` `token_file`) resolve against the directory holding `daemon_config.toml`, so the daemon reads the same files whatever directory it was started from. The flags (`--vaults`, `--tls-cert`, `--tls-key`, `--config`) stay relative to the current directory, like any command-line path. Absolute paths are used as they are, and `~` is not expanded.

> New in 0.4.0 (unreleased): before it, relative paths in the file resolved against the current directory, so an existing config that relied on that now means something else. Until then, clone the repo and build from the checkout: `cargo install --path crates/daemon`

`--read-only` is a one-way switch: it can lock down a permissive config file, but no flag can unlock a file that asked to be read-only. The default bind is loopback on purpose — putting a vault on the network should be something someone typed out.

### The vaults directory

Every `*.toml` under `vaults_path` is opened as a vault, exactly as `nimbus-cli`'s `new` would open it (see [`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md) for the `origin_config` shape). A `crypt` vault served from here draws a warning at startup: the daemon then holds its key and serves plaintext. For storage the server can't read, keep the crypt vault on the client and point its inner origin at a plain vault served here. Each vault config resolves its own relative paths (an `fs` `root`, say) against its own directory, as everywhere else. A vault is addressed in URLs by the `name` **inside** the config, not by its file name.

A config that fails to open is logged and skipped, so one bad file doesn't stop the rest from being served. A config that doesn't parse is logged by file, line and column, never by a value or a line, since it may hold a token; the same goes for `daemon_config.toml` itself. Two configs claiming the same name is fatal — serving one of them at random would silently hand out the wrong data.

Nothing stops a served vault from having an `http` origin of its own, which makes the daemon a proxy in front of another one.

## The API

| Method | Path | What it does |
|--------|------|--------------|
| `GET` | `/health` | Liveness. The only route outside the auth layer. |
| `GET` | `/v` | The vault names being served. |
| `GET` | `/v/{vault}/list/{id}` | The children of `id`, as JSON `Object`s. |
| `GET` | `/v/{vault}/get/{id}` | One object's metadata, as JSON. |
| `GET` | `/v/{vault}/fetch/{id}` | The object's payload, streamed. |
| `PUT` | `/v/{vault}/put/{id}` | Creates the JSON `Object` in the body under directory `id`. |
| `PUT` | `/v/{vault}/send/{id}` | Writes the request body as `id`'s payload (the object must exist). |
| `DELETE` | `/v/{vault}/delete/{id}` | Deletes `id`. |
| `POST` | `/v/{vault}/rename/{id}` | Moves `id` in one step; body `{"destination": "<parent id>", "name": "<new name>"}`, answers with the moved `Object`. Enable it on the client with `rename_url = "/rename/{id}"`. |

Ids arrive percent-encoded (`OriginHTTP` encodes `#`, `?`, `%`, spaces…) and are decoded before use. Object names in a `put` or `rename` body must be a single path component (`object::validate_name`), and anything else is a `400`, so a client can't create files outside the vault. `send` accepts an optional `x-nimbus-modified` header (RFC 3339): the stored file takes that modification time, which is how a `push` through the daemon ends up idempotent. A malformed value is a `400`.

Errors are `{"error": "..."}` with a status: `404` for an unknown vault or a missing object, `400` for a rejected id, name or header, `401` unauthenticated, `403` read-only, `501` when the vault's origin can't do the operation natively (clients then fall back), `409` when the operation would touch an encrypted vault's reserved `.nimbus-crypt` marker, `500` for an origin failure. The `404` matters — `OriginHTTP` turns it back into `VaultError::NotFound`, which is what `Vault::push`/`pull` match on to decide an object needs creating.

The `500` is the one case where the client isn't told what went wrong: the origin's own message can name a path on the serving host, or the upstream a proxying daemon sits in front of. That detail is logged at `error` level instead, so the operator has it and the caller doesn't.

Omitting `{id}` (`/v/photos/list`, `/v/photos/list/`, or `/v/photos/list//`) addresses the vault's **root**, taken from the vault's own config. That's what lets a client whose `root_id` is the default `/` talk to a vault rooted at some opaque origin-specific id.

### Pointing a vault at it

```toml
# ~/.config/.nimbus/vaults/remote.toml
name = "remote"

[origin_config]
type = "http"
base_url   = "http://server:8080/v/photos"
list_url   = "/list/{id}"
fetch_url  = "/fetch/{id}"
get_url    = "/get/{id}"
put_url    = "/put/{id}"
send_url   = "/send/{id}"
delete_url = "/delete/{id}"

# only if the daemon runs with [auth]; a daemon off loopback then needs [tls], so use https:// (below)
[origin_config.auth]
type = "bearer"
token_env = "NIMBUS_TOKEN"
```

```
nimbus> new ~/.config/.nimbus/vaults/remote.toml
nimbus> cd remote
nimbus remote/>> ls
```

When the daemon serves HTTPS (see [TLS](#tls)), use an `https` base URL and tell the vault which CA signed the daemon's certificate:

```toml
[origin_config]
type = "http"
base_url   = "https://server:8443/v/photos"
# ... the same *_url templates as above

[origin_config.tls]
ca_file = "/etc/nimbus/ca.pem"
```

For a self-signed certificate, `pin_sha256` (the certificate's SHA-256 fingerprint) replaces `ca_file`; see [the vault README](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#tls-origin_configtls).

The client side of `[auth]` is [`HttpAuth`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#authenticating-an-http-origin):
the daemon validates credentials, the vault presents them. Point the daemon at a token and the
vault at the same one, and keep the secret out of both config files: `token_file` on the
daemon, `token_env` (or `token_file`) on the vault.

## Authentication

`[auth]` is an internally tagged enum, so adding a scheme is an added variant rather than a breaking change to every existing config. Today:

- `type = "none"` (default) — every request is served as the anonymous identity. The daemon warns at startup if this is combined with a non-loopback bind.
- `type = "bearer"` — a shared secret sent as `Authorization: Bearer <token>`, compared without short-circuiting so a wrong token doesn't leak its correct prefix through the response time.

The bearer secret comes from exactly one of `token` (written in the config) or `token_file` (a file holding only the token); setting both or neither stops startup. `token_file` is the one to reach for, since it keeps the secret out of a config people copy around. Its contents are used as they are, except that trailing newlines are dropped, so `echo s3cr3t > token` works. The path is taken literally, with no `~` expansion, and an empty or missing file is an error naming it. The file is read once at startup: restart the daemon to change the token (SIGHUP reloads only the TLS certificate). nimbus doesn't check the file's permissions, so `chmod 600` it yourself. `--token` replaces the whole `[auth]` table, `token_file` included.

> New in 0.4.0 (unreleased). Until then, clone the repo and build from the checkout: `cargo install --path crates/daemon`

Whatever the scheme, it resolves to an `Identity` that the middleware puts in the request extensions. Per-identity rules (read-only callers, per-vault access) have somewhere to go without changing any handler's signature. `AuthConfig` is the table as written; `AuthConfig::resolve` reads its secret once at startup into the `Auth` the middleware checks. Adding a scheme means: a variant in both, its arm in `resolve` and in `authenticate`, and its label in `describe`.

## TLS

With a `[tls]` table (or `--tls-cert` and `--tls-key`) the daemon serves HTTPS with rustls (ring provider). Without one it serves plain HTTP. A missing, unreadable or mismatched certificate or key stops startup with an error naming the file.

> New in 0.4.0 (unreleased). Until then, clone the repo and build from the checkout: `cargo install --path crates/daemon`

**Safety rule.** The daemon refuses to start when the bind address is not loopback (`0.0.0.0` and `::` count as not loopback), `[auth]` is `bearer`, and there is no `[tls]`: the token would cross the network readable by anyone on the path. Setting `allow_insecure_http = true` in the config file (there is no flag for it) starts anyway, with a warning. With no authentication on a non-loopback address it only warns.

**Shutting down.** Ctrl-C stops accepting and gives in-flight requests 10 s to finish.

**Renewing the certificate.** On Unix, `kill -HUP <pid>` re-reads both files. New connections get the new certificate, open ones keep theirs, and a reload that fails is logged while the old certificate stays in use.

Clients trust the certificate through `[origin_config.tls]` (a private CA file, or the certificate's SHA-256 pin; see the vault README).

### Behind a reverse proxy instead

Keep the daemon on loopback, where plain HTTP and a bearer token are allowed, and let the proxy terminate TLS. With Caddy:

```
nimbus.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

```toml
bind = "127.0.0.1:8080"
[auth]
type = "bearer"
token_file = "/etc/nimbus/token"
```

## Object ids

Ids are origin-specific and mostly opaque, but the filesystem origin resolves them as paths under its root, so the daemon refuses any id with a `..` segment or a null byte before it reaches an origin. That check lives at the HTTP boundary because that's where an id stops being something local code chose and becomes whatever a client sent.

Repeated leading slashes are collapsed rather than refused: they all mean the same thing, and a client whose `root_id` is the default `/` really does send `//name`, because `OriginHTTP::put` joins the destination and the name with a slash. Only one may reach the origin — `OriginFileSystem` strips a single leading slash and would resolve whatever is left as an absolute path on the host.

## Caching

Handlers talk to the vault's `Origin` directly rather than through `Vault`'s own methods, bypassing its in-memory object cache. A daemon lives for weeks and its data changes underneath it; serving a cached `Object` would hand clients stale metadata — and stale metadata is exactly what `Object::compare` uses to decide a file doesn't need syncing.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/LICENSE-APACHE) or
[MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/LICENSE-MIT) at your option — the same terms as the rest of the workspace.
