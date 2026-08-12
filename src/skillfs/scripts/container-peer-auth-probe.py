#!/usr/bin/env python3
"""Independent probe for the SkillFS container peer authentication contract."""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import secrets
import socket
import stat
import sys
import time
from pathlib import Path
from typing import Any

AUTH_VERSION = "1"
FRAME_LIMIT = 4096
BUSINESS_FRAME_LIMIT = 64 * 1024
NONCE_BYTES = 32
MIN_SECRET_BYTES = 32
MAX_SECRET_BYTES = 4096

CONTROL_CLIENT_DOMAIN = b"anolisa.skillfs.control.client.v1"
CONTROL_SERVER_DOMAIN = b"anolisa.skillfs.control.server.v1"
NOTIFY_CLIENT_DOMAIN = b"anolisa.skillfs.notify.client.v1"
NOTIFY_SERVER_DOMAIN = b"anolisa.skillfs.notify.server.v1"


class ProbeError(RuntimeError):
    """The peer did not satisfy the authentication test contract."""


def load_secret(path: str) -> bytes:
    """Load raw secret bytes without following the final path component."""
    secret_path = Path(path)
    if not secret_path.is_absolute():
        raise ProbeError("secret path must be absolute")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(secret_path, flags)
    except OSError as error:
        raise ProbeError(f"cannot open secret: {error}") from error
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise ProbeError("secret must be a regular file")
        secret = os.read(descriptor, MAX_SECRET_BYTES + 1)
    finally:
        os.close(descriptor)
    if not MIN_SECRET_BYTES <= len(secret) <= MAX_SECRET_BYTES:
        raise ProbeError("secret must contain 32 to 4096 raw bytes")
    return secret


def proof(secret: bytes, domain: bytes, nonce: bytes) -> bytes:
    """Calculate one domain-separated HMAC-SHA256 proof."""
    return hmac.new(secret, domain + b"\0" + nonce, hashlib.sha256).digest()


def encode_frame(payload: dict[str, Any]) -> bytes:
    """Encode one compact NDJSON authentication or business frame."""
    return json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n"


def read_frame(
    connection: socket.socket,
    deadline: float,
    limit: int = FRAME_LIMIT,
) -> dict[str, Any]:
    """Read one bounded frame under a wall-clock deadline."""
    data = bytearray()
    while len(data) <= limit:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ProbeError("frame deadline expired")
        connection.settimeout(remaining)
        chunk = connection.recv(1)
        if not chunk:
            raise ProbeError("peer closed the connection")
        data.extend(chunk)
        if chunk == b"\n":
            try:
                payload = json.loads(data.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise ProbeError("peer sent invalid JSON") from error
            if not isinstance(payload, dict):
                raise ProbeError("peer frame must be an object")
            return payload
    raise ProbeError(f"peer frame exceeds {limit} bytes")


def decode_value(payload: dict[str, Any], name: str, size: int) -> bytes:
    """Decode one canonical padded Base64 field of a fixed size."""
    value = payload.get(name)
    if not isinstance(value, str):
        raise ProbeError(f"missing {name}")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, base64.binascii.Error) as error:
        raise ProbeError(f"invalid {name}") from error
    if base64.b64encode(decoded).decode("ascii") != value or len(decoded) != size:
        raise ProbeError(f"non-canonical or incorrectly sized {name}")
    return decoded


def require_frame(payload: dict[str, Any], kind: str, fields: set[str]) -> None:
    """Require an exact authentication frame shape."""
    if (
        payload.get("authVersion") != AUTH_VERSION
        or payload.get("type") != kind
        or set(payload) != fields
    ):
        raise ProbeError(f"unexpected {kind} frame")


def connect(path: str, timeout: float) -> socket.socket:
    """Connect to one Unix socket with a bounded operation timeout."""
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(timeout)
    connection.connect(path)
    return connection


def client_handshake(
    connection: socket.socket,
    secret: bytes,
    timeout: float,
) -> bytes:
    """Authenticate as a control client and return the fresh nonce."""
    deadline = time.monotonic() + timeout
    connection.sendall(encode_frame({"authVersion": AUTH_VERSION, "type": "auth.init"}))
    challenge = read_frame(connection, deadline)
    require_frame(challenge, "auth.challenge", {"authVersion", "type", "nonce"})
    nonce = decode_value(challenge, "nonce", NONCE_BYTES)
    connection.sendall(
        encode_frame(
            {
                "authVersion": AUTH_VERSION,
                "type": "auth.proof",
                "proof": base64.b64encode(
                    proof(secret, CONTROL_CLIENT_DOMAIN, nonce)
                ).decode("ascii"),
            }
        )
    )
    accepted = read_frame(connection, deadline)
    require_frame(accepted, "auth.ok", {"authVersion", "type", "proof"})
    server_proof = decode_value(accepted, "proof", hashlib.sha256().digest_size)
    if not hmac.compare_digest(
        server_proof,
        proof(secret, CONTROL_SERVER_DOMAIN, nonce),
    ):
        raise ProbeError("server proof does not match")
    return nonce


def command_control(arguments: argparse.Namespace) -> None:
    """Authenticate to the control socket and issue one business request."""
    secret = load_secret(arguments.secret)
    request = json.loads(arguments.request)
    if not isinstance(request, dict):
        raise ProbeError("control request must be a JSON object")
    with connect(arguments.socket, arguments.timeout) as connection:
        nonce = client_handshake(connection, secret, arguments.timeout)
        connection.sendall(encode_frame(request))
        response = read_frame(
            connection,
            time.monotonic() + arguments.timeout,
            BUSINESS_FRAME_LIMIT,
        )
    print(
        json.dumps(
            {"nonce": base64.b64encode(nonce).decode("ascii"), "response": response}
        )
    )


def command_plain(arguments: argparse.Namespace) -> None:
    """Pass only when an HMAC control server rejects a plain request."""
    with connect(arguments.socket, arguments.timeout) as connection:
        connection.sendall(arguments.request.encode("utf-8") + b"\n")
        connection.settimeout(arguments.timeout)
        response = connection.recv(FRAME_LIMIT + 1)
    if response:
        raise ProbeError(f"plain request unexpectedly received data: {response!r}")
    print("PASS: plain control request rejected")


def command_replay(arguments: argparse.Namespace) -> None:
    """Pass only when a proof from an earlier connection is rejected."""
    secret = load_secret(arguments.secret)
    with connect(arguments.socket, arguments.timeout) as first:
        first.sendall(encode_frame({"authVersion": AUTH_VERSION, "type": "auth.init"}))
        first_challenge = read_frame(first, time.monotonic() + arguments.timeout)
        first_nonce = decode_value(first_challenge, "nonce", NONCE_BYTES)
        stale_proof = proof(secret, CONTROL_CLIENT_DOMAIN, first_nonce)

    # The single-request listener may still be retiring the abandoned first
    # handshake. Wait for its bounded deadline before opening the replay.
    time.sleep(min(arguments.timeout, 0.2))
    with connect(arguments.socket, arguments.timeout) as second:
        second.sendall(encode_frame({"authVersion": AUTH_VERSION, "type": "auth.init"}))
        second_challenge = read_frame(second, time.monotonic() + arguments.timeout)
        second_nonce = decode_value(second_challenge, "nonce", NONCE_BYTES)
        if hmac.compare_digest(first_nonce, second_nonce):
            raise ProbeError("server reused a challenge nonce")
        second.sendall(
            encode_frame(
                {
                    "authVersion": AUTH_VERSION,
                    "type": "auth.proof",
                    "proof": base64.b64encode(stale_proof).decode("ascii"),
                }
            )
        )
        second.settimeout(arguments.timeout)
        response = second.recv(FRAME_LIMIT + 1)
    if response:
        raise ProbeError(f"replayed proof unexpectedly received data: {response!r}")
    print("PASS: fresh nonce observed and replayed proof rejected")


def command_slow(arguments: argparse.Namespace) -> None:
    """Pass only when a slow partial auth frame cannot retain the server."""
    frame = encode_frame({"authVersion": AUTH_VERSION, "type": "auth.init"})
    started = time.monotonic()
    with connect(arguments.socket, arguments.bound) as connection:
        rejected = False
        for byte in frame:
            if time.monotonic() - started > arguments.bound:
                break
            try:
                connection.sendall(bytes([byte]))
                connection.settimeout(0.01)
                if connection.recv(1) == b"":
                    rejected = True
                    break
            except socket.timeout:
                pass
            except (BrokenPipeError, ConnectionResetError):
                rejected = True
                break
            time.sleep(arguments.interval)
    elapsed = time.monotonic() - started
    if not rejected or elapsed > arguments.bound:
        raise ProbeError(f"slow auth was not rejected within {arguments.bound}s")
    print(f"PASS: slow auth rejected after {elapsed:.3f}s")


def server_handshake(
    connection: socket.socket,
    secret: bytes,
    timeout: float,
    wrong_server_proof: bool,
) -> None:
    """Authenticate one SkillFS notify client."""
    deadline = time.monotonic() + timeout
    init = read_frame(connection, deadline)
    require_frame(init, "auth.init", {"authVersion", "type"})
    nonce = secrets.token_bytes(NONCE_BYTES)
    connection.sendall(
        encode_frame(
            {
                "authVersion": AUTH_VERSION,
                "type": "auth.challenge",
                "nonce": base64.b64encode(nonce).decode("ascii"),
            }
        )
    )
    client = read_frame(connection, deadline)
    require_frame(client, "auth.proof", {"authVersion", "type", "proof"})
    client_proof = decode_value(client, "proof", hashlib.sha256().digest_size)
    if not hmac.compare_digest(
        client_proof,
        proof(secret, NOTIFY_CLIENT_DOMAIN, nonce),
    ):
        raise ProbeError("notify client proof does not match")
    server_proof = proof(secret, NOTIFY_SERVER_DOMAIN, nonce)
    if wrong_server_proof:
        server_proof = bytes([server_proof[0] ^ 1]) + server_proof[1:]
    connection.sendall(
        encode_frame(
            {
                "authVersion": AUTH_VERSION,
                "type": "auth.ok",
                "proof": base64.b64encode(server_proof).decode("ascii"),
            }
        )
    )


def command_notify_server(arguments: argparse.Namespace) -> None:
    """Accept and record one mutually authenticated notify v2 request."""
    secret = load_secret(arguments.secret)
    socket_path = Path(arguments.socket)
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    if socket_path.exists():
        socket_path.unlink()
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        listener.bind(str(socket_path))
        os.chmod(socket_path, 0o600)
        listener.listen(1)
        listener.settimeout(arguments.timeout)
        print(f"READY: {socket_path}", flush=True)
        connection, _ = listener.accept()
        with connection:
            server_handshake(
                connection,
                secret,
                arguments.timeout,
                arguments.wrong_server_proof,
            )
            if arguments.wrong_server_proof:
                connection.settimeout(0.5)
                try:
                    unexpected = connection.recv(FRAME_LIMIT + 1)
                except socket.timeout:
                    unexpected = b""
                if unexpected:
                    raise ProbeError(
                        "client sent business data after a wrong server proof"
                    )
                print("PASS: wrong notify server proof blocked business data")
                return
            request = read_frame(
                connection,
                time.monotonic() + arguments.timeout,
                BUSINESS_FRAME_LIMIT,
            )
            if request.get("method") != "skill_ledger.skillfs_notify_change":
                raise ProbeError("unexpected notify business method")
            Path(arguments.output).write_text(
                json.dumps(request, indent=2) + "\n",
                encoding="utf-8",
            )
            connection.sendall(
                encode_frame(
                    {
                        "ok": True,
                        "data": {"schemaVersion": 2, "accepted": True},
                    }
                )
            )
            print("PASS: authenticated notify v2 recorded")
    finally:
        listener.close()
        try:
            socket_path.unlink()
        except FileNotFoundError:
            pass


def build_parser() -> argparse.ArgumentParser:
    """Build the probe command-line parser."""
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    def add_socket_timeout(subparser: argparse.ArgumentParser) -> None:
        subparser.add_argument("--socket", required=True)
        subparser.add_argument("--timeout", type=float, default=6.0)

    control = subparsers.add_parser("control", help="authenticate and send a request")
    add_socket_timeout(control)
    control.add_argument("--secret", required=True)
    control.add_argument("--request", required=True)
    control.set_defaults(handler=command_control)

    plain = subparsers.add_parser("plain", help="assert plain control rejection")
    add_socket_timeout(plain)
    plain.add_argument(
        "--request",
        default='{"schemaVersion":"1","method":"ping"}',
    )
    plain.set_defaults(handler=command_plain)

    replay = subparsers.add_parser("replay", help="assert stale proof rejection")
    add_socket_timeout(replay)
    replay.add_argument("--secret", required=True)
    replay.set_defaults(handler=command_replay)

    slow = subparsers.add_parser("slow", help="assert total handshake deadline")
    slow.add_argument("--socket", required=True)
    slow.add_argument("--interval", type=float, default=1.0)
    slow.add_argument("--bound", type=float, default=7.0)
    slow.set_defaults(handler=command_slow)

    notify = subparsers.add_parser("notify-server", help="serve one notify request")
    add_socket_timeout(notify)
    notify.add_argument("--secret", required=True)
    notify.add_argument("--output", required=True)
    notify.add_argument("--wrong-server-proof", action="store_true")
    notify.set_defaults(handler=command_notify_server)
    return parser


def main() -> int:
    """Run one probe command and return a shell-friendly status."""
    arguments = build_parser().parse_args()
    try:
        arguments.handler(arguments)
    except (OSError, ProbeError, json.JSONDecodeError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
