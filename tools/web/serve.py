#!/usr/bin/env python3
"""Serves the web build on 127.0.0.1, behind the VPS's Caddy and Cloudflare: index.html is
checked every load, a build's folder (b/HASH/) is kept for good, and the fonts and
dictionaries for a day.

    python3 serve.py PORT FOLDER
"""
import functools
import http.server
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {**http.server.SimpleHTTPRequestHandler.extensions_map, '.wasm': 'application/wasm'}

    def end_headers(self):
        path = self.path.split('?')[0]
        if path.startswith('/b/'):
            cache = 'public, max-age=31536000, immutable'
        elif path in ('/', '/index.html'):
            cache = 'no-cache'
        else:
            cache = 'public, max-age=86400'
        self.send_header('Cache-Control', cache)
        super().end_headers()


if __name__ == '__main__':
    port, folder = int(sys.argv[1]), sys.argv[2]
    handler = functools.partial(Handler, directory=folder)
    http.server.ThreadingHTTPServer(('127.0.0.1', port), handler).serve_forever()
