#!/usr/bin/env python3
"""Exercise a real server using the bundled client and optionally the reference client."""
import argparse
import importlib
import queue
import re
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples"))
from client import Slmp3E


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--reference-dir", type=Path)
    parser.add_argument("--rust-client", type=Path)
    args = parser.parse_args()
    classes = [("bundled", Slmp3E)]
    if args.reference_dir:
        sys.path.insert(0, str(args.reference_dir.resolve()))
        slmp = importlib.import_module("slmp")

        class Reference(Slmp3E):
            def __init__(self, host, port, transport="tcp", iqr=False, timeout=5):
                self.iqr = iqr
                # This profile supplies the 3E envelope. raw_command lets us test
                # the server's custom writable SM0 and either device-spec format.
                self.client = slmp.SlmpClient(
                    host, port, transport=transport, plc_profile="melsec:iq-f", timeout=timeout,
                    default_target=slmp.SlmpTarget(network=0, station=0xFF, module_io=0x03FF, multidrop=0),
                )
                self.client.connect()

            def request(self, command, subcommand, payload):
                return self.client.raw_command(command, subcommand=subcommand, payload=payload).data

            def close(self):
                self.client.close()

        classes.append(("plc-comm-slmp-python", Reference))

    with tempfile.TemporaryDirectory(prefix="sm0sh-interop-") as directory:
        config = Path(directory) / "sm0sh.toml"
        config.write_text('bind = "127.0.0.1:0"\ntransport = "both"\nsd_words = 32\ncommand_timeout_ms = 1000\n')
        process = subprocess.Popen([str(args.binary.resolve()), "--config", str(config)], stderr=subprocess.PIPE, text=True)
        try:
            started = queue.Queue()
            reader = threading.Thread(target=lambda: started.put(process.stderr.readline()), daemon=True)
            reader.start()
            line = started.get(timeout=10)
            match = re.search(r"listening on 127\.0\.0\.1:(\d+)", line)
            if not match:
                raise RuntimeError("server failed to start: " + line)
            port = int(match[1])
            for name, client_class in classes:
                for transport in ["tcp", "udp"]:
                    for iqr in [False, True]:
                        client = client_class("127.0.0.1", port, transport, iqr)
                        try:
                            output, status = client.run(["/usr/bin/printf", "%s|%s|%s", "日本語", "", "two words"])
                            assert output == "日本語||two words", (name, output)
                            assert status[0:3] == [2, 0, 0], status
                            output, status = client.run(["/bin/sh", "-c", "printf out; printf err >&2; exit 7"])
                            assert output == "outerr" and status[:2] == [3, 7], (output, status)
                            output, status = client.run(["/bin/sh", "-c", "head -c 10000 /dev/zero | tr '\\000' x"])
                            assert output == "x" * 63 and status[0] == 2, (output, status)
                            assert client.read_words(0x91, 0, 1)[0] & 0x10
                            # A shorter next result must not leak any tail of the old log.
                            output, status = client.run(["/bin/echo", "ok"])
                            assert output == "ok\n" and status[3:5] == [3, 0], (output, status)
                            assert client.read_words(0xA9, 2, 30) == [0] * 30
                            print(f"PASS {name} {transport} {'iQR' if iqr else 'Q/L'}", flush=True)
                        finally:
                            client.close()
            client = Slmp3E("127.0.0.1", port)
            try:
                output, status = client.run(["/does-not-exist/sm0sh"])
                assert status[0] == 5 and "sm0sh" in output
                output, status = client.run(["/bin/sleep", "2"])
                assert status[0] == 6 and "timed out" in output
                print("PASS spawn failure and timeout", flush=True)
            finally:
                client.close()
            if args.rust_client:
                subprocess.run([str(args.rust_client.resolve()), str(port)], check=True, timeout=30)
        finally:
            process.terminate()
            _, errors = process.communicate(timeout=5)
            if process.returncode != 0:
                raise RuntimeError(f"server exit {process.returncode}: {errors}")


if __name__ == "__main__":
    main()
