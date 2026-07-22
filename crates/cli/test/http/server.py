#!/usr/bin/env python3
"""
Servidor de prueba para el origin "http" de nimbus (crates/vault/src/origin/http.rs).
Sirve el directorio ./data como si fuera un vault remoto, hablando el mismo JSON
que espera OriginHTTP (el enum `Object` de object.rs, con tag externo:
{"Leaf": {...}} / {"Branch": {...}}).

Uso:
    python3 server.py [puerto]   # default 8787
"""
import http.server
import json
import os
import shutil
import socketserver
import sys
import urllib.parse
from datetime import datetime, timezone

DATA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8787


def id_to_path(obj_id: str) -> str:
    rel = obj_id.strip("/")
    return os.path.join(DATA_DIR, rel) if rel else DATA_DIR


def describe(path: str, obj_id: str) -> dict:
    name = os.path.basename(path.rstrip("/")) or "/"
    st = os.stat(path)
    modified = datetime.fromtimestamp(st.st_mtime, tz=timezone.utc).isoformat()
    if os.path.isdir(path):
        return {
            "Branch": {
                "name": name,
                "id": obj_id,
                "meta": {"size": None, "content_type": None, "modified": modified, "extra": {}},
                "children": None,
            }
        }
    return {
        "Leaf": {
            "name": name,
            "id": obj_id,
            "meta": {"size": st.st_size, "content_type": None, "modified": modified, "extra": {}},
        }
    }


class Handler(http.server.BaseHTTPRequestHandler):
    def _send_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_bytes(self, status, data, content_type="application/octet-stream"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _obj_id(self, prefix):
        return urllib.parse.unquote(self.path[len(prefix):])

    def do_GET(self):
        if self.path.startswith("/list/"):
            obj_id = self._obj_id("/list/")
            path = id_to_path(obj_id)
            if not os.path.isdir(path):
                return self._send_json(404, {"error": "not found"})
            rel = obj_id.strip("/")
            children = []
            for name in sorted(os.listdir(path)):
                child_id = f"{rel}/{name}" if rel else name
                children.append(describe(os.path.join(path, name), child_id))
            return self._send_json(200, children)

        if self.path.startswith("/get/"):
            obj_id = self._obj_id("/get/")
            path = id_to_path(obj_id)
            if not os.path.exists(path):
                return self._send_json(404, {"error": "not found"})
            return self._send_json(200, describe(path, obj_id))

        if self.path.startswith("/fetch/"):
            obj_id = self._obj_id("/fetch/")
            path = id_to_path(obj_id)
            if not os.path.isfile(path):
                return self._send_json(404, {"error": "not found"})
            with open(path, "rb") as f:
                return self._send_bytes(200, f.read())

        return self._send_json(404, {"error": "unknown route"})

    def do_PUT(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""

        if self.path.startswith("/put/"):
            destination = self._obj_id("/put/")
            try:
                obj = json.loads(body)
            except json.JSONDecodeError:
                return self._send_json(400, {"error": "invalid json"})
            variant, fields = next(iter(obj.items()))
            name = fields["name"]
            target = os.path.join(id_to_path(destination), name)
            if variant == "Branch":
                os.makedirs(target, exist_ok=True)
            else:
                os.makedirs(os.path.dirname(target), exist_ok=True)
                open(target, "ab").close()
            self.send_response(201)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        if self.path.startswith("/send/"):
            obj_id = self._obj_id("/send/")
            path = id_to_path(obj_id)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as f:
                f.write(body)
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        return self._send_json(404, {"error": "unknown route"})

    def do_DELETE(self):
        if self.path.startswith("/delete/"):
            obj_id = self._obj_id("/delete/")
            path = id_to_path(obj_id)
            if not os.path.exists(path):
                return self._send_json(404, {"error": "not found"})
            if os.path.isdir(path):
                shutil.rmtree(path)
            else:
                os.remove(path)
            self.send_response(204)
            self.end_headers()
            return
        return self._send_json(404, {"error": "unknown route"})

    def log_message(self, fmt, *args):
        sys.stderr.write("[http-vault] " + (fmt % args) + "\n")


def main():
    os.makedirs(DATA_DIR, exist_ok=True)
    with socketserver.TCPServer(("127.0.0.1", PORT), Handler) as httpd:
        print(f"sirviendo {DATA_DIR} en http://127.0.0.1:{PORT}")
        httpd.serve_forever()


if __name__ == "__main__":
    main()
