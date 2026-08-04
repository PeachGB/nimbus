# vaults de prueba

Un config por cada `type` de `OriginConfig` (`crates/vault/src/config.rs`), para
probar el REPL a mano contra cada uno. Los comandos de abajo se tipean en el
prompt del REPL (`nimbus />>`), con rutas relativas a la raíz del repo — que es
desde donde arranca `cargo run -p nimbus-cli`. Las rutas dentro de los `.toml` están
hardcodeadas en absoluto (`/home/arian/projects/rust/nimbus/crates/cli/test/...`);
si movés esta carpeta o el repo hay que actualizarlas.

Importante: `nimbus-cli` persiste `vault_configs` en
`$XDG_STATE_HOME/nimbus/session.toml` (o `~/.local/state/nimbus/session.toml`
si no seteás `XDG_STATE_HOME`). Para no ensuciar tu sesión real mientras
probás esto, corré el binario con un `XDG_STATE_HOME` temporal:

```sh
export XDG_STATE_HOME=$(mktemp -d)
cargo run -p nimbus-cli
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
new crates/cli/test/fs/fs.toml
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
new crates/cli/test/command/command.toml
select cmd-vault
ls
```

**Las limitaciones que documentaba esta sección ya están arregladas** (ver
`crates/vault/README.md#command-templating`):

- `put_cmd`/`send_cmd` ahora sí interpolan los `extras` configurados, así que
  pueden usar `{root}`/`{helper}` como cualquier otro template. Antes sólo
  interpolaban el `meta.extra` del objeto, por eso acá están hardcodeados.
- `send` ahora corre bajo `sh -c` como el resto, así que `send_cmd` puede ser un
  comando con argumentos y placeholders — ya no tiene que ser la ruta pelada a un
  script. `command/send.sh` vuelca stdin a un archivo fijo
  (`data/last-upload.bin`), que era el único `send` posible antes; un `send`
  correcto por objeto tendría que escribir en `{root}/data/{id}`.
- Se agregó `{kind}` (`leaf`/`branch`), sin el cual `put_cmd` no podía distinguir
  crear un archivo de crear un directorio (y `mkdir` era imposible).

Los `.toml` de esta carpeta no se actualizaron para aprovechar nada de eso.

## http/ — `type = "http"`

Vault contra un server HTTP de juguete (`server.py`, sólo stdlib) que sirve
`http/data/` con el mismo contrato JSON que `OriginHTTP` espera. Hay que
levantarlo antes:

```sh
python3 crates/cli/test/http/server.py   # puerto 8787 por default
```

y en otra terminal (o en el mismo REPL, en paralelo):

```
new crates/cli/test/http/http.toml
select http-vault
ls
```

`server.py` implementa `list`/`get`/`fetch`/`put`/`send`/`delete` completos, así
que sirve para probar el ciclo entero `put`/`get`/`delete` sin tocar el vault
`LOCAL`.

## vault-of-vault/ — `type = "vault"`

Envuelve `fs/fs.toml` como origin de otro vault (`OriginVault`), para probar
que anidar vaults funciona.

```
new crates/cli/test/vault-of-vault/nested.toml
select nested-vault
ls
cd docs
ls
```
