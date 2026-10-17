#!/usr/bin/env python3
"""Dependency-free sm0sh client. Binary 3E, no automatic request retries."""
from __future__ import annotations

import argparse
import socket
import struct
import time

ROUTE = bytes.fromhex("00 ff ff 03 00")


def encode_argv(argv: list[str]) -> tuple[list[int], list[int]]:
    if not argv or not argv[0] or len(argv) > 127:
        raise ValueError("provide a command and at most 126 arguments")
    d, w = [], []
    for element in argv:
        if "\0" in element:
            raise ValueError("NUL is not allowed in argv")
        data = element.encode("utf-8") or b"\0\0"
        data += b"\0" * (len(data) % 2)
        d.extend(struct.unpack(f"<{len(data) // 2}H", data))
        if len(d) > 8192:
            raise ValueError("argv exceeds the default 8192 D words")
        w.append(len(d) - 1)
    return d, w + [65535]


class Slmp3E:
    def __init__(self, host: str, port: int, transport: str = "tcp", iqr: bool = False, timeout: float = 5):
        self.transport, self.iqr, self.timeout = transport, iqr, timeout
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM if transport == "tcp" else socket.SOCK_DGRAM)
        self.socket.settimeout(timeout)
        self.socket.connect((host, port))

    def close(self):
        self.socket.close()

    def _exact(self, size: int, deadline: float) -> bytes:
        data = bytearray()
        while len(data) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("SLMP response timeout")
            self.socket.settimeout(remaining)
            part = self.socket.recv(size - len(data))
            if not part:
                raise ConnectionError("server closed the connection")
            data.extend(part)
        return bytes(data)

    def request(self, command: int, subcommand: int, payload: bytes) -> bytes:
        body = struct.pack("<HHH", 16, command, subcommand) + payload
        frame = b"\x50\0" + ROUTE + struct.pack("<H", len(body)) + body
        deadline = time.monotonic() + self.timeout
        self.socket.settimeout(self.timeout)
        if self.transport == "tcp":
            self.socket.sendall(frame)
            header = self._exact(9, deadline)
            response = header + self._exact(struct.unpack_from("<H", header, 7)[0], deadline)
        else:
            self.socket.send(frame)
            response = self.socket.recv(65536)
        if len(response) < 11 or response[:7] != b"\xd0\0" + ROUTE:
            raise ValueError("invalid SLMP 3E response")
        if len(response) != 9 + struct.unpack_from("<H", response, 7)[0]:
            raise ValueError("invalid SLMP response length")
        code = struct.unpack_from("<H", response, 9)[0]
        if code:
            raise RuntimeError(f"SLMP end code 0x{code:04X}")
        return response[11:]

    def spec(self, code: int, start: int, count: int) -> bytes:
        address = struct.pack("<IH", start, code) if self.iqr else start.to_bytes(3, "little") + bytes([code])
        return address + struct.pack("<H", count)

    def write_words(self, code: int, words: list[int], start: int = 0):
        for offset in range(0, len(words), 960):
            part = words[offset:offset + 960]
            self.request(0x1401, 2 if self.iqr else 0,
                         self.spec(code, start + offset, len(part)) + struct.pack(f"<{len(part)}H", *part))

    def read_words(self, code: int, start: int, count: int) -> list[int]:
        result = []
        for offset in range(0, count, 960):
            size = min(960, count - offset)
            data = self.request(0x0401, 2 if self.iqr else 0, self.spec(code, start + offset, size))
            if len(data) != size * 2:
                raise ValueError("unexpected word response length")
            result.extend(struct.unpack(f"<{size}H", data))
        return result

    def trigger(self):
        self.request(0x1401, 3 if self.iqr else 1, self.spec(0x91, 0, 1) + b"\x10")

    def run(self, argv: list[str], timeout: float = 35) -> tuple[str, list[int]]:
        d, w = encode_argv(argv)
        previous = self.read_words(0xB5, 5, 2)
        self.write_words(0xA8, d)
        self.write_words(0xB4, w)
        self.trigger()  # A timeout here is ambiguous; do not automatically retry.
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = self.read_words(0xB5, 0, 8)
            if status[5:7] != previous and status[0] >= 2:
                length = status[3] | (status[4] << 16)
                words = self.read_words(0xA9, 0, (length + 1) // 2)
                output = struct.pack(f"<{len(words)}H", *words)[:length].decode("utf-8")
                if self.read_words(0xB5, 5, 2) != status[5:7]:
                    raise RuntimeError("another writer replaced the result while reading")
                return output, status
            time.sleep(0.02)
        raise TimeoutError("command completion timeout; check SM0/SW before resending")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=5000)
    parser.add_argument("--transport", choices=["tcp", "udp"], default="tcp")
    parser.add_argument("--iqr", action="store_true", help="use subcommands 0002/0003 inside the 3E frame")
    parser.add_argument("--dry-run", action="store_true", help="print D/W values without connecting")
    parser.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    d, w = encode_argv(argv)
    if args.dry_run:
        for i, word in enumerate(d):
            print(f"D{i} = 0x{word:04X}")
        for i, word in enumerate(w):
            print(f"W{i:X} = {word}")
        return 0
    client = Slmp3E(args.host, args.port, args.transport, args.iqr)
    try:
        output, status = client.run(argv)
        print(output, end="")
        return 0 if status[0] == 2 else 1
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
