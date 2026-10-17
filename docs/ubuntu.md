# Running on Ubuntu

## Local Trial

Build with Cargo on Rust 1.85+.
The standard `cargo` package on Ubuntu 24.04 may be older, so verify `rustc --version`.
Use the stable toolchain via Rustup, or newer package versions provided by Ubuntu.
Example using `cargo-1.91` on Ubuntu 24.04:

```bash
sudo apt update
sudo apt install --no-install-recommends cargo-1.91 rustc-1.91 gcc libc6-dev
cargo-1.91 build --release --locked
```

When using Rustup, standard `cargo build --release --locked` is sufficient.
The initial build requires network access to crates.io. For air-gapped environments, transfer a pre-built executable.
Runtime does not require the Rust toolchain, Python, or reference clients.
If using a pre-packaged binary, verify its architecture and test environment in `validation.md`.

```bash
./target/release/sm0sh --config examples/sm0sh.toml --check-config
./target/release/sm0sh --config examples/sm0sh.toml
```

From another terminal:

```bash
python3 examples/client.py -- /usr/bin/printf '%s\n' 'hello PLC'
python3 examples/client.py -- /bin/sh -c 'printf out; printf err >&2'
```

## PLC Network Configuration

If Ubuntu is `192.168.10.20` and the PLC is `192.168.10.10`, configure as follows.
Port 5000 is an example; any unprivileged port matching the PLC configuration can be used.

```toml
bind = "192.168.10.20:5000"
transport = "tcp"
allowed_ips = ["192.168.10.10"]
```

Ensure the PLC side is also set to **3E Binary**.
If the PLC lacks dedicated instructions for SLMP batch writes targeting remote D/W/SM devices, construct 3E request frames using socket communication functions. Specific instructions and configuration items vary depending on the PLC CPU and network modules.
The included examples are for wire-format connection verification, not ladder programs for specific CPUs.

Example UFW firewall rule for TCP:

```bash
sudo ufw allow from 192.168.10.10 to 192.168.10.20 port 5000 proto tcp
```

If UDP is selected, configure corresponding UDP firewall rules.
Exposing ports to the public internet is not intended.

## Service Setup / Daemonization

Install the built binary.
This script does not overwrite existing configurations, nor does it start the service.

```bash
sudo sh deploy/install.sh target/release/sm0sh
sudoedit /etc/sm0sh/sm0sh.toml
sudo -u sm0sh /usr/local/bin/sm0sh --config /etc/sm0sh/sm0sh.toml --check-config
sudo systemctl enable --now sm0sh
```

Commands run under the privileges of the dedicated `sm0sh` user.
By default in the systemd service configuration, writable locations are limited to `/var/lib/sm0sh` and private temporary directories.
User home directories are hidden. Place scripts to be executed in locations like `/var/lib/sm0sh`.
If accessing other directories is required by your application, adjust `ReadWritePaths` or `ProtectHome` in the service file.
The service configuration does not create isolated sandboxes on a per-command basis.

```bash
sudo systemctl status sm0sh
sudo journalctl -u sm0sh -f
sudo systemctl stop sm0sh
```

The server daemon log records target process path, execution count, status, and exit code.
Command stdout/stderr is stored in SD registers. Command string details are not enumerated in the server log.
SIGINT/SIGTERM stops accepting new traffic and terminates active child process groups.
Systemd's `KillMode=control-group` is also configured to ensure all processes under the service are cleanly shut down.

## Configuration Options

| Option | Default | Description |
| --- | --- | --- |
| bind | `127.0.0.1:5000` | IP address and port. Resolved once at startup as a numeric address |
| transport | `tcp` | `tcp`, `udp`, `both` |
| allowed_ips | 2 loopback IPs | Exact match list of allowed IPs. Cannot be empty |
| d_words | 8192 | 1–65535 |
| w_words | 128 | 2–1024, including the terminating marker |
| sd_words | 8192 | 1–65535 |
| command_timeout_ms | 30000 | 1–86,400,000 ms |
| connection_timeout_ms | 30000 | 100–3,600,000 ms, applied separately to request reception and response sending |
| max_connections | 16 | Number of TCP connections. 1–1024 |
| working_directory | `.` | Working directory for commands. Relative paths from startup cwd are allowed |
| environment | PATH/LANG | Environment variables passed to child processes. Server process environment is not inherited |

When `[environment]` is specified, that table defines the child process environment variables.
Include PATH and LANG in the table as needed. Configuration changes take effect upon restarting the service.
Unknown options, out-of-range values, or non-existent working directories are treated as fatal startup errors.
