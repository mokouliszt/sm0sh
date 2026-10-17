# Mailbox Specification (ABI 1)

This device map is specific to sm0sh. It is independent of the semantics of special relays and special registers on MELSEC CPUs. SLMP device codes are utilized as communication addresses.

## Wire Format

| Item | Supported |
| --- | --- |
| Frame | 3E binary, Request `50 00`, Response `D0 00` |
| Transport | TCP (default), UDP, configurable to both |
| Command | Batch Read `0x0401`, Batch Write `0x1401`, Block Read `0x0406`, Block Write `0x1406` |
| Subcommand | Word `0000/0002`, Bit `0001/0003` |
| Q/L device specification | 3-byte head number + 1-byte code |
| iQ-R device specification | 4-byte head number + 2-byte code |
| Code | D=`A8`, W=`B4`, SD=`A9`, SM=`91`, SW=`B5` |
| Numerical byte order | Little-endian |
| Bit packing | 2 points per byte. Earlier point in high nibble. 1-point write of SM0 ON is `10` |
| Batch request limits | 960 points for words, 7168 points for bits. Physical memory size bounds also apply |

Network number, PC number, Unit I/O number, and Station number are echoed in responses.
Frames are not routed to other stations. Standard route value is `00 FF FF 03 00`.
Monitoring timer value is received as part of the frame, but is not used as a command execution timeout.
SLMP responses are returned immediately upon request acceptance; execution time is bounded by `command_timeout_ms`.

TCP handles fragmented and concatenated packet streams. If the full frame from header to body cannot be received within the connection timeout, the connection is closed. The connection is also closed if the header is invalid or if the body length is outside the 6–4096 byte range. If a header is valid but the request body is malformed, an SLMP end code is returned.
UDP requires each request to be contained in a single complete datagram. Incomplete, oversized, or requests with trailing extraneous data are dropped.

## Block Access

Block Read (`0406`) and Block Write (`1406`) accept subcommands `0000` (Q/L) and `0002` (iQ-R).
Subcommands `0001` and `0003` are not used for block access.

The payload begins with two one-byte counts: word-device blocks, then bit-device blocks.
Specify all word-device blocks (D/W/SD/SW) first, followed by bit-device blocks (SM).
Each block specifies the head device number, device code, and a two-byte point count,
using the same device specification and little-endian integers as batch access.
Each point in an SM block represents 16 bits, not one bit.

For a read, send only block specifications. The response concatenates the data of each
block in request order, with no per-block headers. SM values are packed into 16-bit words.
For a write, place each block's word data immediately after its specification, before
the next block specification. SM write data is also word-packed, not nibble-packed.

| Limit | Q/L (`0000`) | iQ-R (`0002`) |
| --- | --- | --- |
| Total blocks | 1–120 | 1–60 |
| Total read points | At most 960 | At most 960 |
| Write points, including block overhead | `total_points + 4 * blocks <= 960` | `total_points + 9 * blocks <= 960` |

Every block must contain at least one point and fit within its allocated device area.
These limits follow the SLMP Reference Manual, SH-080956ENG-M, sections Read Block and Write Block.

Writes accept D/W and at most one SM block. That SM block must start at SM0, contain one
word, and have the value `0x0000` or `0x0001`; SM1–SM15 remain read-only.
All blocks are validated before any device is changed. An invalid later block rejects
the entire write without clearing the previous result or starting a command.
Overlapping D/W blocks are applied in request order; the last write wins.
An SM0 request is processed after all D/W updates, so D/W staging and execution can
share one request. Writes while SM0 is ON still return `0xCEE0`.

A block read observes all requested regions under one mailbox lock. For example, SW0–SW7
and SD0–SD951 fit in one read (960 words total). Choose the SD range before sending:
the SW log length returned in that response cannot change the size of the same request.
The single-mailbox execution and SD reset rules below also apply to block access.

## D and W

Default is 8192 words for D, and 128 words for W.

1. Store `argv[0]` (the executable path/name) starting at D0.
2. Characters are encoded in UTF-8. Bytes are ordered with the lower byte first, then the upper byte in each 16-bit word.
3. Pad each string element with zeros up to the word boundary. The next element starts at the next word.
4. Set `W[i]` to the **last D index (inclusive)** of `argv[i]`.
5. Write `65535` (`0xFFFF`) to the W register immediately following the last element.

Element 0 begins at D0. Element i begins at `W[i-1] + 1`.
Command IDs, element count headers, and fixed-length argument slots are not used.
End D numbers in W must be strictly monotonically increasing and within the allocated range of D.

An empty argument is allocated 1 word of zero (`0x0000`). Trailing zero bytes are stripped, and internal NUL bytes within an element cause an error.
Trailing spaces are preserved. An empty string for the executable name, missing W termination marker, out-of-range or non-ascending end numbers, or invalid UTF-8 result in runtime input errors.

W device numbers are formatted in hexadecimal (W9 is followed by WA; WF is followed by W10).
Stored end numbers are 0-based indices into D. For example, the end value for D100 is integer 100 (`0x0064`).
Because `0xFFFF` is reserved as the termination marker, D supports up to 65,535 words (D0–D65534).
W can be configured from 2 to 1024 words, reserving 1 word for the final marker.

Always write up to the W termination marker on each execution. Even if values from previous executions remain in W, anything past the marker is ignored.
Large D data sets must be split into chunks of 960 words or fewer. Confirm the response of the final write before turning SM0 ON.

## SM

SM allocates 16 points.

| Device | Meaning | PLC Operation |
| --- | --- | --- |
| SM0 | Accepted / Running | Can be requested ON only when idle |
| SM1 | Completed (held until next acceptance) | Read only |
| SM2 | Error / Abnormal completion | Read only |
| SM3 | Timed out | Read only |
| SM4 | Log truncated | Read only |
| SM5–SM15 | Reserved, always OFF | Read only |

Triggering execution via a 1-point bit write to SM0 is recommended.
A 1-word read starting at SM0 returns SM0–SM15 packed into 16 bits.
A 1-word write starting at SM0 with value 0 or 1 is accepted, but will not alter the upper 15 bits.
Other SM word writes are rejected.

When an ON request is accepted while SM0 is OFF, the following operations occur atomically in memory:

- The entire allocated SD area is cleared to 0.
- SM1–SM15 and previous result flags are reset.
- The execution count is incremented, SW0 is set to Running (1), and SM0 turns ON.
- Command and arguments are materialized from the current D/W and dispatched to the execution queue.

Writes to D/W and ON/OFF operations on SM0 while running are rejected with `0xCEE0`.
Rejections do not reset the running log or flags.
Writing OFF to SM0 when idle does nothing. There is no cancellation mechanism over the network.

Upon completion, SD, SW, and SM1–SM4 are updated before SM0 turns OFF.
This completion sequence also executes on input errors or spawn failures.
Because short-lived commands may finish before the PLC observes SM0 ON, completion can also be determined via SM1 or by checking the execution count and SW0.

## SW

SW consists of 8 words, all read-only from the PLC.

| Device | Content |
| --- | --- |
| SW0 | Execution state (see table below) |
| SW1 | Process exit code (`0xFFFF` if unavailable) |
| SW2 | Terminating signal number (0 for normal termination or unavailable) |
| SW3 | SD log size in bytes, lower 16 bits |
| SW4 | SD log size in bytes, upper 16 bits |
| SW5 | Execution count, lower 16 bits |
| SW6 | Execution count, upper 16 bits |
| SW7 | Mailbox ABI version (currently 1) |

| SW0 | Meaning |
| ---: | --- |
| 0 | Initial state |
| 1 | Accepted / Running |
| 2 | Succeeded (process exit code 0) |
| 3 | Process exited with non-zero status or signal |
| 4 | Failed to parse command/arguments from D/W |
| 5 | Failed to spawn executable |
| 6 | Execution timed out |
| 7 | Aborted (e.g., service shutdown) |
| 8 | I/O error (reading process output, waiting for process, etc.) |

The execution count starts at 0 upon server startup, increments on each accepted request, and wraps at 32 bits. It is not a persistent unique ID across restarts.
On signal termination or timeout, do not rely on SW1 alone; consult SW0 and SW2 as well.

## SD Log

Default is 8192 words. stdout and stderr are merged into the same stream and recorded in the order received.
Due to process-side buffering, the observed output order may differ from the actual event order.
Standard input is connected to `/dev/null`.

Output is treated as UTF-8; invalid bytes or NUL characters are replaced with the replacement character.
Space for a terminating NUL byte is always preserved at the end. The maximum byte count is `sd_words * 2 - 1`.
UTF-8 character boundaries are never split; when truncation occurs, SM4 is turned ON.
Process output is continually drained and discarded after truncation, preventing the child process from stalling due to filled output buffers.

Until the log is finalized, the entire SD area remains zero. It is not an append log updated in real time.
Results are retained until the next execution acceptance. Acceptance of the next request does not wait for log reads; therefore, limit writers to a single client and only send the next request after retrieving results.

The read sequence is: perform a batch read of SW0–SW7, verify the completed state, and read `SW3 + (SW4 << 16)` bytes of SD in chunks of 960 words or fewer.
Finally, checking that SW5/SW6 have not changed confirms that results were not overwritten by another operation during the read.
An empty log has 0 bytes, so no SD read requests are needed.

## End Codes and Retries

| SLMP End Code | Meaning in sm0sh |
| --- | --- |
| `0000` | Device operation accepted |
| `C050` | Invalid body length or write data length |
| `C051` / `C052` | Invalid word count / bit count |
| `C056` | Out of allocated device range |
| `C059` | Unsupported command / subcommand |
| `C05B` | Unsupported device type, access mode, or write target |
| `C060` | Invalid bit data nibble value |
| `CEE0` | Execution in progress, or execution service unavailable |

An SM0 write response of `0000` only signifies acceptance, not successful command completion.
Process outcome must be determined via SW and SD.

SLMP 3E requests contain no deduplication ID. Never mechanically resend execution requests after a network disconnection or lost response. Check SW execution count and state first, and allow the application logic to decide whether re-execution is safe.
