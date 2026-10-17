# sm0sh

An SLMP 3E server written in Rust that executes Ubuntu commands via SLMP.

Write command and argument strings to D, and write their respective end D device numbers to W.
Setting SM0 to ON executes the command, stores logs in SD, and turns SM0 back OFF.
SM represents virtual special relays and SD represents virtual special registers on the sm0sh side.

## Quickstart & Connection Verification

The bundled `sm0sh` binary is verified on Ubuntu 24.04 / x86_64.
You can run it without installing Rust.

```bash
chmod +x sm0sh
./sm0sh
```

By default without configuration, it listens on `127.0.0.1:5000/TCP`.
Verify the connection from another terminal using the Python 3 client:

```bash
python3 examples/client.py -- /bin/echo 'hello PLC'
```

## Connecting from a PLC

Change `bind` in `examples/sm0sh.toml` to Ubuntu's Ethernet IP, and `allowed_ips` to the PLC's IP.

```toml
bind = "192.168.10.20:5000"
allowed_ips = ["192.168.10.10"]
```

Start the server specifying the configuration file:

```bash
./sm0sh --config examples/sm0sh.toml
```

TCP is the default transport, but `transport = "udp"` or `"both"` can also be selected.
See [docs/ubuntu.md](docs/ubuntu.md) for daemonizing the service on Ubuntu.

## Example: Sending `echo hello`

Strings are UTF-8 encoded. Two bytes are stored per 16-bit word starting from the lower byte.
For elements with an odd number of bytes, pad the final upper byte with 0.

| Device | Value | Content |
| --- | --- | --- |
| D0 | `0x6365` | `ec` |
| D1 | `0x6F68` | `ho` |
| D2 | `0x6568` | `he` |
| D3 | `0x6C6C` | `ll` |
| D4 | `0x006F` | `o` + 0 |
| W0 | `1` | Command spans D0–D1 |
| W1 | `4` | 1st argument spans D2–D4 |
| W2 | `65535` | End of element table |

Store the end D number of each element in W. The first element starts at D0, and subsequent elements start at `(previous end + 1)`.
Device numbers for W are in hexadecimal notation. The values stored are 0-based D numbers; for example, if an element ends at D100, store 100 (`0x0064`).

The PLC sequence of operations is as follows:

1. Perform batch writes for D and W (including the terminating `65535`).
2. Turn SM0 ON via a 1-point bit write.
3. Wait for SM0 to turn OFF or SM1 to turn ON.
4. Read SW status and log length, then retrieve the log starting from SD0.

The response to the SM0 write request only indicates acceptance. It does not wait for command completion.
Writing to D/W or re-triggering SM0 during execution returns SLMP end code `0xCEE0`.

Block Write (`1406`) can combine D, W, and SM0 in one request.
Block Read (`0406`) can retrieve SW and a selected SD range in one response.
See [Block Access](docs/protocol.md#block-access) for formats and limits.

## Devices

| Area | Default Points | Purpose |
| --- | ---: | --- |
| D0–D8191 | 8192 words | UTF-8 string data for command and arguments |
| W0–W7F | 128 words | End D numbers for each element, ending with 65535 |
| SM0 | 1 bit | ON from execution acceptance to completion |
| SM1–SM4 | 4 bits | Complete, Error, Timeout, Log Truncated |
| SD0–SD8191 | 8192 words | Combined stdout and stderr UTF-8 log |
| SW0–SW7 | 8 words | Status, exit code, log length, execution count, ABI version |

When SM0 execution is accepted, the entire allocated SD area is cleared.
SD, SW, and SM1+ are read-only. Logs and execution results are published together upon completion.
Detailed register specifications are documented in [docs/protocol.md](docs/protocol.md).

## Arguments and Shell Syntax

Each element delimited by W is passed directly to the OS as command/arguments.
Arguments containing spaces, Japanese characters, and empty strings are supported. Quotes are not re-interpreted.

To use pipes or redirection, invoke a shell explicitly:

```bash
python3 examples/client.py -- /bin/sh -c 'printf hello | tr a-z A-Z'
```

In this case, the 3 elements are `/bin/sh`, `-c`, and `printf hello | tr a-z A-Z`.

To inspect D/W values before sending:

```bash
python3 examples/client.py --dry-run -- /bin/echo 'hello PLC'
```

## Scope and Operations

- 3E binary frame, TCP/UDP, Batch Read `0401`, Batch Write `1401`, Block Read `0406`, Block Write `1406`.
- Q/L format `0000/0001` and iQ-R format `0002/0003` device specifications.
- Word access for D/W/SD/SW, bit access for SM, and word access for SM0.
- Single shared mailbox. Ensure only one client writes from D/W preparation to result retrieval.
- Memory resides in RAM only. Initialized on restart; commands are not automatically re-executed.
- Default log capacity is up to 16,383 bytes. Excess output is discarded, and SM4 signals truncation.

SLMP provides no authentication or encryption for this use case. Allowed clients can execute arbitrary commands with the privileges of the service user. Operate over a dedicated PLC network under a dedicated user account. IP allowlisting does not substitute for authentication, and commands are not sandboxed.

Over UDP, resending an SM0 ON request after a lost response may cause the command to execute again. Use TCP for operations with side effects, and verify status before deciding to re-execute when a response is uncertain.

4E frames, ASCII frames, random access, and CPU/label operations are unsupported.
See [docs/validation.md](docs/validation.md) for validated environments and results.

## Development

Building from source requires Rust 1.85+:

```bash
cargo build --release --locked
./target/release/sm0sh --config examples/sm0sh.toml
```

Formatting and test execution:

```bash
cargo fmt --check
cargo test --locked
```

## License

[MIT License](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party dependency licenses.
