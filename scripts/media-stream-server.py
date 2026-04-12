#!/usr/bin/env python3
"""Lightweight media streaming HTTP server with Range request support.

Serves MEDIA_ROOT on MEDIA_BIND_ADDR:MEDIA_PORT with:
- HTTP Range requests (206 Partial Content) for VLC seeking
- os.sendfile() for zero-copy transfers on macOS
- Directory listing and proper MIME types
- Logging to ~/logs/media-stream-server.log
"""

import os
import signal
import sys
import logging
import mimetypes
import html
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import unquote, quote

BIND_ADDR = os.environ.get("MEDIA_BIND_ADDR", "0.0.0.0")
PORT = int(os.environ.get("MEDIA_PORT", "9876"))
ROOT = "/Volumes/Jellyfin Media/MEDIA"
LOG_FILE = os.path.expanduser("~/logs/media-stream-server.log")

# Extra MIME types for video files
mimetypes.add_type("video/x-matroska", ".mkv")
mimetypes.add_type("video/mp4", ".mp4")
mimetypes.add_type("video/x-msvideo", ".avi")
mimetypes.add_type("video/mp2t", ".ts")
mimetypes.add_type("video/webm", ".webm")

logging.basicConfig(
    filename=LOG_FILE, level=logging.INFO,
    format="%(asctime)s %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
)
log = logging.getLogger("media-stream")


class MediaServer(HTTPServer):
    allow_reuse_address = True


class MediaHandler(BaseHTTPRequestHandler):
    server_version = "MediaStreamServer/1.0"

    def log_message(self, fmt, *args):
        log.info("%s %s", self.address_string(), fmt % args)

    def translate_path(self):
        path = unquote(self.path.split("?", 1)[0].split("#", 1)[0])
        parts = [p for p in path.split("/") if p and p != ".."]
        return os.path.join(ROOT, *parts)

    def do_HEAD(self):
        self._handle(head_only=True)

    def do_GET(self):
        self._handle(head_only=False)

    def _handle(self, head_only=False):
        fpath = self.translate_path()

        if not os.path.exists(fpath):
            self.send_error(404)
            return

        if os.path.isdir(fpath):
            self._serve_directory(fpath, head_only)
            return

        self._serve_file(fpath, head_only)

    def _serve_directory(self, dpath, head_only):
        rel = os.path.relpath(dpath, ROOT)
        title = "/" if rel == "." else f"/{rel}/"
        entries = sorted(os.listdir(dpath))
        lines = [f"<html><head><title>{html.escape(title)}</title></head>",
                 "<body><h1>%s</h1><ul>" % html.escape(title)]
        if rel != ".":
            lines.append('<li><a href="../">../</a></li>')
        for name in entries:
            full = os.path.join(dpath, name)
            display = name + ("/" if os.path.isdir(full) else "")
            href = quote(name) + ("/" if os.path.isdir(full) else "")
            lines.append(f'<li><a href="{href}">{html.escape(display)}</a></li>')
        lines.append("</ul></body></html>")
        body = "\n".join(lines).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", len(body))
        self.end_headers()
        if not head_only:
            self.wfile.write(body)

    def _serve_file(self, fpath, head_only):
        try:
            fsize = os.path.getsize(fpath)
        except OSError:
            self.send_error(404)
            return

        ctype, _ = mimetypes.guess_type(fpath)
        if ctype is None:
            ctype = "application/octet-stream"

        range_header = self.headers.get("Range")
        if range_header:
            self._serve_range(fpath, fsize, ctype, range_header, head_only)
        else:
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", fsize)
            self.send_header("Accept-Ranges", "bytes")
            self.end_headers()
            if not head_only:
                self._send_file(fpath, 0, fsize)

    def _serve_range(self, fpath, fsize, ctype, range_header, head_only):
        try:
            spec = range_header.replace("bytes=", "").strip()
            if spec.startswith("-"):
                suffix = int(spec[1:])
                start = max(0, fsize - suffix)
                end = fsize - 1
            elif spec.endswith("-"):
                start = int(spec[:-1])
                end = fsize - 1
            else:
                parts = spec.split("-", 1)
                start = int(parts[0])
                end = int(parts[1])
        except (ValueError, IndexError):
            self.send_error(416, "Invalid Range")
            return

        if start > end or start >= fsize:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{fsize}")
            self.end_headers()
            return

        end = min(end, fsize - 1)
        length = end - start + 1

        self.send_response(206)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", length)
        self.send_header("Content-Range", f"bytes {start}-{end}/{fsize}")
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        if not head_only:
            self._send_file(fpath, start, length)

    def _send_file(self, fpath, offset, length):
        try:
            with open(fpath, "rb") as f:
                fd_in = f.fileno()
                fd_out = self.wfile.fileno()
                sent = 0
                while sent < length:
                    chunk = min(length - sent, 1024 * 1024)
                    n = os.sendfile(fd_out, fd_in, offset + sent, chunk)
                    if n == 0:
                        break
                    sent += n
        except (BrokenPipeError, ConnectionResetError):
            pass
        except OSError:
            # Fallback to regular read/write if sendfile fails
            try:
                with open(fpath, "rb") as f:
                    f.seek(offset)
                    remaining = length
                    while remaining > 0:
                        chunk = f.read(min(remaining, 65536))
                        if not chunk:
                            break
                        self.wfile.write(chunk)
                        remaining -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass


def main():
    server = MediaServer((BIND_ADDR, PORT), MediaHandler)
    log.info("Starting media server on %s:%d serving %s", BIND_ADDR, PORT, ROOT)

    def shutdown(signum, frame):
        log.info("Shutting down (signal %d)", signum)
        server.server_close()
        sys.exit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    print(f"Serving {ROOT} on http://{BIND_ADDR}:{PORT}")
    server.serve_forever()


if __name__ == "__main__":
    main()
