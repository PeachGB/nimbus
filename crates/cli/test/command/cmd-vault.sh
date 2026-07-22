#!/bin/sh
# helper para el vault de tipo "command": get/list contra ./data, emitiendo
# el JSON que espera OriginCommand (mismo shape que Object en object.rs).
set -eu

ROOT="$(cd "$(dirname "$0")/data" && pwd)"
OP="$1"
ID="${2:-}"
REL="${ID#/}"

if [ -z "$REL" ]; then
    TARGET="$ROOT"
else
    TARGET="$ROOT/$REL"
fi

describe() {
    path="$1"
    id="$2"
    name=$(basename "$path")
    if [ -d "$path" ]; then
        jq -n --arg name "$name" --arg id "$id" \
            '{"Branch": {"name": $name, "id": $id, "meta": {"size": null, "content_type": null, "modified": null, "extra": {}}, "children": null}}'
    else
        size=$(stat -c%s "$path")
        jq -n --arg name "$name" --arg id "$id" --argjson size "$size" \
            '{"Leaf": {"name": $name, "id": $id, "meta": {"size": $size, "content_type": null, "modified": null, "extra": {}}}}'
    fi
}

case "$OP" in
    get)
        [ -e "$TARGET" ] || { echo "not found: $ID" >&2; exit 1; }
        describe "$TARGET" "$ID"
        ;;
    list)
        [ -d "$TARGET" ] || { echo "not a directory: $ID" >&2; exit 1; }
        entries="[]"
        for f in "$TARGET"/*; do
            [ -e "$f" ] || continue
            name=$(basename "$f")
            if [ -z "$REL" ]; then child_id="$name"; else child_id="$REL/$name"; fi
            entries=$(echo "$entries" | jq --argjson obj "$(describe "$f" "$child_id")" '. + [$obj]')
        done
        echo "$entries"
        ;;
    *)
        echo "usage: cmd-vault.sh {get|list} <id>" >&2
        exit 1
        ;;
esac
