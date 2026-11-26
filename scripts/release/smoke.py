"""Startup smoke test for a packaged adp-server release.

Usage: python3 smoke.py <extracted-release-dir>

Runs two minimal loopback Nostr relays on ephemeral ports, starts the packaged
`adp-server` with throwaway credentials and a temporary data directory, then
checks /health, /.well-known/adp and the kind:30403 announcement the server
must publish at startup. Also checks that `adp-refunds` runs and fails closed
without configuration. No public relay, network service or real key is used;
everything is removed afterwards. Standard library only.
"""

import base64
import hashlib
import json
import os
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class Relay:
    """Accepts EVENT (answers OK) and REQ (answers EOSE) over WebSocket."""

    def __init__(self):
        self.events = []
        self.listener = socket.create_server(("127.0.0.1", 0))
        self.url = "ws://127.0.0.1:%d" % self.listener.getsockname()[1]
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self):
        while True:
            try:
                connection, _ = self.listener.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(connection,), daemon=True).start()

    @staticmethod
    def _read_exact(connection, size):
        data = b""
        while len(data) < size:
            chunk = connection.recv(size - len(data))
            if not chunk:
                raise ConnectionError("closed")
            data += chunk
        return data

    @staticmethod
    def _send(connection, opcode, payload):
        header = bytes([0x80 | opcode])
        if len(payload) < 126:
            header += bytes([len(payload)])
        elif len(payload) < 65536:
            header += bytes([126]) + struct.pack(">H", len(payload))
        else:
            header += bytes([127]) + struct.pack(">Q", len(payload))
        connection.sendall(header + payload)

    def _serve(self, connection):
        try:
            request = b""
            while b"\r\n\r\n" not in request:
                request += connection.recv(4096)
            headers = {}
            for line in request.decode("latin-1").split("\r\n")[1:]:
                if ":" in line:
                    key, value = line.split(":", 1)
                    headers[key.strip().lower()] = value.strip()
            accept = base64.b64encode(
                hashlib.sha1((headers["sec-websocket-key"] + WS_GUID).encode()).digest()
            ).decode()
            connection.sendall(
                (
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                    "Connection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n" % accept
                ).encode()
            )
            while True:
                first, second = self._read_exact(connection, 2)
                opcode, length = first & 0x0F, second & 0x7F
                if length == 126:
                    length = struct.unpack(">H", self._read_exact(connection, 2))[0]
                elif length == 127:
                    length = struct.unpack(">Q", self._read_exact(connection, 8))[0]
                mask = self._read_exact(connection, 4) if second & 0x80 else b"\0\0\0\0"
                payload = bytes(
                    byte ^ mask[index % 4]
                    for index, byte in enumerate(self._read_exact(connection, length))
                )
                if opcode == 0x8:
                    self._send(connection, 0x8, payload[:2])
                    return
                if opcode == 0x9:
                    self._send(connection, 0xA, payload)
                    continue
                if opcode != 0x1:
                    continue
                message = json.loads(payload)
                if message[0] == "EVENT":
                    self.events.append(message[1])
                    reply = ["OK", message[1]["id"], True, ""]
                elif message[0] == "REQ":
                    reply = ["EOSE", message[1]]
                else:
                    continue
                self._send(connection, 0x1, json.dumps(reply).encode())
        except (ConnectionError, OSError, KeyError, ValueError, IndexError):
            pass
        finally:
            connection.close()


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def get_json(url):
    with urllib.request.urlopen(url, timeout=5) as response:
        return response.status, json.load(response)


def main(release_dir):
    server_binary = os.path.join(release_dir, "adp-server")
    refunds_binary = os.path.join(release_dir, "adp-refunds")
    relays = [Relay(), Relay()]
    port = free_port()
    base = "http://127.0.0.1:%d" % port
    with tempfile.TemporaryDirectory() as data:
        environment = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "RUST_LOG": "warn",
            "ADP_SERVER_URL": base,
            "ADP_BIND_ADDR": "127.0.0.1:%d" % port,
            "ADP_DB_PATH": os.path.join(data, "adp.db"),
            "ADP_FILES_DIR": os.path.join(data, "files"),
            "ADP_RELAYS": ",".join(relay.url for relay in relays),
            "ADP_PUBLISHER_RELAYS": "disabled",
            # Throwaway, never-published smoke-test identity.
            "ADP_OPERATOR_NSEC": secrets.token_hex(32),
            "ADP_MASTER_KEY": secrets.token_hex(32),
        }
        server = subprocess.Popen([server_binary], cwd=data, env=environment)
        try:
            deadline = time.monotonic() + 60
            while True:
                if server.poll() is not None:
                    raise SystemExit("adp-server exited during startup: %s" % server.returncode)
                try:
                    status, health = get_json(base + "/health")
                    if status == 200:
                        break
                except OSError:
                    pass
                if time.monotonic() > deadline:
                    raise SystemExit("adp-server did not become healthy")
                time.sleep(0.2)
            print("health:", health)
            status, info = get_json(base + "/.well-known/adp")
            assert status == 200 and info.get("adp_version"), info
            pubkey = info["pubkey"]
            assert len(pubkey) == 64 and int(pubkey, 16) >= 0, info
            print("well-known:", info)
            announcements = [
                event
                for relay in relays
                for event in relay.events
                if event.get("kind") == 30403 and event.get("pubkey") == pubkey
            ]
            assert announcements, "no kind:30403 announcement reached the relays"
            urls = {tag[1] for event in announcements for tag in event["tags"] if tag[0] == "url"}
            assert base in urls, urls
            print("announcement published to", len(announcements), "relay(s)")
            assert os.path.exists(environment["ADP_DB_PATH"]), "database not created in data dir"
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()

    refunds = subprocess.run(
        [refunds_binary, "list"],
        env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")},
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert refunds.returncode != 0 and "ADP_SERVER_URL is required" in refunds.stderr, (
        refunds.returncode,
        refunds.stderr,
    )
    print("adp-refunds fails closed without configuration")
    print("SMOKE TEST PASSED")


if __name__ == "__main__":
    main(sys.argv[1])
