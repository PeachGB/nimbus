# nimbus-daemon

Serves a directory of vault configs over HTTP, so another nimbus can mount them through the [`http` origin](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md). It's the other half of `OriginHTTP`: one machine holds the data, the rest talk to it with the same `ls`/`get`/`put`/`push`/`pull` they'd use on a local folder.

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
token = "s3cr3t"
```

| Key | Flag | Default |
|-----|------|---------|
| `vaults_path` | `--vaults <DIR>` | — (required) |
| `bind` | `--bind <ADDR>` | `127.0.0.1:8080` |
| `read_only` | `--read-only` | `false` |
| `[auth]` | `--token <TOKEN>` | `type = "none"` |

`--read-only` is a one-way switch: it can lock down a permissive config file, but no flag can unlock a file that asked to be read-only. The default bind is loopback on purpose — putting a vault on the network should be something someone typed out.

### The vaults directory

Every `*.toml` under `vaults_path` is opened as a vault, exactly as `nimbus-cli`'s `new` would open it (see [`crates/vault/README.md`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md) for the `origin_config` shape). A vault is addressed in URLs by the `name` **inside** the config, not by its file name.

A config that fails to open is logged and skipped, so one bad file doesn't stop the rest from being served. Two configs claiming the same name is fatal — serving one of them at random would silently hand out the wrong data.

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

Errors are `{"error": "..."}` with a status: `404` for an unknown vault or a missing object, `400` for a rejected id, `401` unauthenticated, `403` read-only, `500` for an origin failure. The `404` matters — `OriginHTTP` turns it back into `VaultError::NotFound`, which is what `Vault::push`/`pull` match on to decide an object needs creating.

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

# only if the daemon is running with [auth]
[origin_config.auth]
type = "bearer"
token_env = "NIMBUS_TOKEN"
```

```
nimbus> new ~/.config/.nimbus/vaults/remote.toml
nimbus> cd remote
nimbus remote/>> ls
```

The client side of `[auth]` is [`HttpAuth`](https://github.com/PeachGB/nimbus/blob/main/crates/vault/README.md#authenticating-an-http-origin):
the daemon validates credentials, the vault presents them. Point the daemon at a token and the
vault at the same one, and keep the secret in an environment variable rather than in either
file.

## Authentication

`[auth]` is an internally tagged enum, so adding a scheme is an added variant rather than a breaking change to every existing config. Today:

- `type = "none"` (default) — every request is served as the anonymous identity. The daemon warns at startup if this is combined with a non-loopback bind.
- `type = "bearer"` — a shared secret sent as `Authorization: Bearer <token>`, compared without short-circuiting so a wrong token doesn't leak its correct prefix through the response time.

Whatever the scheme, it resolves to an `Identity` that the middleware puts in the request extensions. Per-identity rules (read-only callers, per-vault access) have somewhere to go without changing any handler's signature. Adding a scheme means: a variant in `AuthConfig`, its arm in `authenticate`, and its label in `describe`.

There is no TLS. Run it behind a reverse proxy, or over a private network/tunnel, if the traffic leaves the host — a bearer token on plaintext HTTP is readable by anything on the path.

## Object ids

Ids are origin-specific and mostly opaque, but the filesystem origin resolves them as paths under its root, so the daemon refuses any id with a `..` segment or a null byte before it reaches an origin. That check lives at the HTTP boundary because that's where an id stops being something local code chose and becomes whatever a client sent.

Repeated leading slashes are collapsed rather than refused: they all mean the same thing, and a client whose `root_id` is the default `/` really does send `//name`, because `OriginHTTP::put` joins the destination and the name with a slash. Only one may reach the origin — `OriginFileSystem` strips a single leading slash and would resolve whatever is left as an absolute path on the host.

## Caching

Handlers talk to the vault's `Origin` directly rather than through `Vault`'s own methods, bypassing its in-memory object cache. A daemon lives for weeks and its data changes underneath it; serving a cached `Object` would hand clients stale metadata — and stale metadata is exactly what `Object::changed` uses to decide a file doesn't need syncing.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/LICENSE-APACHE) or
[MIT license](https://github.com/PeachGB/nimbus/blob/main/crates/daemon/LICENSE-MIT) at your option — the same terms as the rest of the workspace.
