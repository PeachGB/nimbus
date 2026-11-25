#!/bin/sh
# Ejemplo de `send_cmd` atomico: el payload llega por stdin. Se escribe en un
# archivo temporal del mismo directorio y solo se mueve sobre el destino si
# stdin termino bien. Si nimbus aborta la transferencia, mata este proceso
# antes de cerrar stdin, asi que el `mv` nunca corre y el destino queda intacto.
#
# Uso en el config: send_cmd = "{root}/send.sh {root}/data/{id}"
set -eu
dest="${1:-$(dirname "$0")/data/last-upload.bin}"
tmp="$(dirname "$dest")/.nimbus-tmp.$(basename "$dest").$$"
trap 'rm -f "$tmp"' EXIT
cat > "$tmp"
mv "$tmp" "$dest"
trap - EXIT
