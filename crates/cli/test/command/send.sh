#!/bin/sh
# `send_cmd` se invoca como `sh <este-script>` (un solo argv, sin `-c` y sin
# placeholders disponibles -- ver README.md de test/ para el detalle), asi
# que no puede saber a que objeto corresponde el payload. Solo vuelca stdin
# a un archivo fijo para poder confirmar manualmente que el pipe funciona.
cat > "$(dirname "$0")/data/last-upload.bin"
