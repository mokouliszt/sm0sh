# Validation Results

Conducted on sm0sh 0.1.0.

## Environment

- Ubuntu 24.04.3 LTS, x86_64.
- rustc 1.91.1 / cargo 1.91.1 (Ubuntu package distribution).
- Bundled binary is a release build; Rust standard library is statically linked.
- Dynamically linked against system `libc`, `libm`, `libgcc_s`, and dynamic linker.
- ARM64, Ubuntu versions prior to 22.04, and communication with physical PLCs have not been validated.

## Results

| Check | Result |
| --- | --- |
| Formatting check (`cargo fmt`) | Passed |
| Clippy (all targets, warnings as errors) | Passed, 0 warnings |
| Release build with Cargo.lock | Passed |
| Mailbox and frame contract tests | 10 passed |
| Linux process execution tests | 6 passed |
| Real-socket TCP/UDP tests | 5 passed |
| Bundled Python client | Passed across all 4 combinations: TCP/UDP × Q/L / iQ-R |
| Startup, config validation, and SIGTERM shutdown | Passed |
| Install script syntax | Passed |

## Verified Behaviors

- Writing to D and W alone does not trigger command execution.
- W end D numbers properly delimit commands and arguments. Japanese characters, spaces, and empty arguments are preserved.
- The entire SD area is cleared when SM0 is accepted. Shorter subsequent logs do not leave trailing remnants from previous runs.
- Re-triggering and writing to D/W during command execution are rejected.
- When two connections send execution requests concurrently, only one is accepted.
- stdout/stderr, exit code, spawn failures, and timeouts are accurately reflected in the result registers.
- Large volume outputs are drained and discarded without hanging due to log capacity limits.
- Log truncation does not cut multi-byte UTF-8 sequences in the middle; room for a terminating NUL byte is maintained in SD.
- TCP fragmentation and multiple requests concatenated in a single send are properly handled.
- Invalid body lengths, bit values, or out-of-bounds ranges do not result in partial memory mutations.
- UDP requests with trailing extraneous data are rejected.
- Disallowed source IP addresses are rejected.
- Normal child process groups are terminated upon timeout, service stop, or premature parent exit.

This is not a per-command isolation mechanism for programs that detach themselves to another session using `setsid` or similar calls. Systemd cgroup-level teardown is utilized upon service shutdown.

## Re-running Verification

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/interop_python.py --binary target/release/sm0sh
```
