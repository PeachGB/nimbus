# vaults de prueba

Un config por cada `type` de `OriginConfig` (`crates/vault/src/config.rs`), para
probar el REPL a mano contra cada uno. Las rutas dentro de los `.toml` están
hardcodeadas en absoluto (`/home/arian/Projects/lambda/nimbus/nimbus/crates/cli/test/...`);
si movés esta carpeta hay que actualizarlas.

Importante: `nimbus-cli` persiste `vault_configs` en
`$XDG_STATE_HOME/nimbus/session.toml` (o `~/.local/state/nimbus/session.toml`
si no seteás `XDG_STATE_HOME`). Para no ensuciar tu sesión real mientras
probás esto, corré el binario con un `XDG_STATE_HOME` temporal:

```sh
cd nimbus/nimbus
export XDG_STATE_HOME=$(mktemp -d)
./target/debug/nimbus-cli
```

También tené en cuenta que el vault `LOCAL` (usado por `put`/`get`/`push`/`pull`)
apunta a tu `$HOME` por defecto — no hace falta para probar `ls`/`cd`/`select`
en los vaults de abajo, pero si querés probar `put`/`get` sin arriesgar
archivos reales, poné en `~/.config/.nimbus/cli_config.toml`:

```toml
default_local_vault = true
local_vault_path = "/ruta/a/una/carpeta/sandbox"
```

## fs/ — `type = "fs"`

Vault sobre un directorio real (`fs/data/`, con un archivo y un subdirectorio).
No necesita nada corriendo.

```
new test/fs/fs.toml
select fs-vault
ls
cd docs
ls
```

## command/ — `type = "command"`

Vault sobre `command/data/`, wrappeado en un script (`cmd-vault.sh`) que
convierte listados/stats reales en el JSON que espera `OriginCommand`
(`list_cmd`/`get_cmd` deben imprimir el mismo shape que el enum `Object`).

```
new test/command/command.toml
select cmd-vault
ls
```

Limitación real del origin, no de este test setup: `OriginCommand::put`/`send`
interpolan `{id}`/`{name}`/etc. desde los metadatos del objeto
(`bootstrap_cmd_object`), pero **no** desde `extras` — así que `put_cmd`/
`send_cmd` no pueden usar `{root}` (por eso están hardcodeados acá). Peor
todavía: `send` invoca el comando como `sh <cmd>` (un solo argv, sin `-c`,
ver `origin/command.rs::send`), así que `send_cmd` tiene que ser literalmente
la ruta a un script sin espacios ni placeholders — no hay forma de que ese
script sepa a qué objeto corresponde el payload. `command/send.sh` sólo
vuelca stdin a un archivo fijo (`data/last-upload.bin`) para confirmar que el
pipe llega; no es un `send` correcto por-objeto. Si te importa `put`/`send`
sobre este origin en serio, ese es el próximo bug a mirar en
`crates/vault/src/origin/command.rs`.

## http/ — `type = "http"`

Vault contra un server HTTP de juguete (`server.py`, sólo stdlib) que sirve
`http/data/` con el mismo contrato JSON que `OriginHTTP` espera. Hay que
levantarlo antes:

```sh
python3 test/http/server.py        # puerto 8787 por default
```

y en otra terminal (o en el mismo REPL, en paralelo):

```
new test/http/http.toml
select http-vault
ls
```

`server.py` implementa `list`/`get`/`fetch`/`put`/`send`/`delete` completos
(a diferencia del origin `command`, acá `put`/`send` sí quedan bien
resueltos), así que es el mejor lugar para probar el ciclo completo
`put`/`get`/`delete` sin tocar el vault `LOCAL`.

## vault-of-vault/ — `type = "vault"`

Envuelve `fs/fs.toml` como origin de otro vault (`OriginVault`), para probar
que anidar vaults funciona.

```
new test/vault-of-vault/nested.toml
select nested-vault
ls
cd docs
ls
```
