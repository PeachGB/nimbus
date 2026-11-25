#!/bin/sh
# An atomic `send_cmd` example: the payload arrives on stdin. It is written to
# a temp file in the same directory and only moved over the destination if
# stdin ended cleanly. If nimbus aborts the transfer, it kills this process
# before closing stdin, so the `mv` never runs and the destination is left intact.
#
# Usage in the config: send_cmd = "{root}/send.sh {root}/data/{id}"
set -eu
dest="${1:-$(dirname "$0")/data/last-upload.bin}"
tmp="$(dirname "$dest")/.nimbus-tmp.$(basename "$dest").$$"
trap 'rm -f "$tmp"' EXIT
cat > "$tmp"
mv "$tmp" "$dest"
trap - EXIT
