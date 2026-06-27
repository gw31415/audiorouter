# audiorouter v0.1 Specification

`audiorouter` is a macOS-first command-line audio router. It reads a TOML configuration file, opens named CoreAudio devices, remaps/mixes audio channels in real time, and writes the mixed result into output devices such as BlackHole.

The v0.1 scope is intentionally small: **named devices + routes only**. There are no explicit buses, sends, scenes, plugins, UI, daemon mode, subcommands, or HAL driver in v0.1.

## 1. Goals

v0.1 must support:

1. A subcommand-free UNIX-style CLI:
   - `audiorouter` runs with the default config.
   - `audiorouter CONFIG` runs with a specified config file.
   - `audiorouter --check [CONFIG]` validates config and device availability.
   - `audiorouter --list-devices` lists available devices and does not read config.
   - `audiorouter --print-config-path [CONFIG]` prints the resolved config path and exits.
2. A simple TOML model:
   - `[[devices]]`: named real audio devices.
   - `[[routes]]`: channel mappings from one device to another.
3. Multiple inputs mixed into one output.
4. One input routed to multiple outputs.
5. Channel remapping, including mono-to-stereo by repeating `from_channels`.
6. Per-route gain in decibels.
7. Optional per-output limiter flag.
8. Clean Ctrl-C (SIGINT) shutdown while running.
9. Clear error message when the default config path does not exist and no `CONFIG` is supplied.

v0.1 does **not** need to support:

- CLI subcommands such as `run`, `check`, `devices`, or `init`.
- `--config`; config selection is the positional `CONFIG` argument.
- Generating or printing example configs from the CLI, including `--example-config`.
- Creating or modifying config files.
- HAL AudioServerPlugIn virtual device implementation.
- Creating or destroying Aggregate Devices.
- Installing BlackHole.
- Capturing per-application audio via Audio Process Tap.
- Config hot reload.
- Recording to file.
- Network streaming.
- GUI/menu bar app.
- Persistent daemon/service mode.
- Per-route EQ/compressor/noise gate.
- Arbitrary graph cycles.
- Sample-rate conversion beyond fail-fast validation.

## 2. Platform

v0.1 targets macOS.

The implementation may compile on other platforms if CPAL supports them, but the product contract for v0.1 is macOS/CoreAudio. If platform-specific assumptions are needed, guard them with `cfg(target_os = "macos")`. On non-macOS platforms, the binary must either refuse to start with a clear error message at runtime or produce a compile-time error — do not silently produce wrong behavior.

## 3. CLI interface

Use `clap` derive or an equivalent clear command parser.

There are no subcommands in v0.1.

```text
Usage:
  audiorouter [OPTIONS] [CONFIG]

Arguments:
  [CONFIG]
      TOML configuration file to read.
      If omitted, audiorouter reads the default XDG config path.

Options:
  -n, --check
      Validate configuration and device availability, then exit.

  -l, --list-devices
      List available audio input/output devices, then exit.
      Does not read CONFIG.

      --print-config-path
      Print the resolved configuration path, then exit.
      If CONFIG is supplied, print the resolved CONFIG path.

  -q, --quiet
      Suppress non-error output.

  -v, --verbose
      Print extra diagnostics. Repeat for more detail.
      -v:   debug-level tracing (open/close device, route resolution)
      -vv:  trace-level (per-callback timing, underrun counts)

  -h, --help
      Print help.

  -V, --version
      Print version.
```

### 3.1 Mode selection

The program has four modes:

| Invocation | Mode | Reads config? | Starts audio? |
|---|---|---:|---:|
| `audiorouter` | run | yes | yes |
| `audiorouter CONFIG` | run | yes | yes |
| `audiorouter --check` | check | yes | no |
| `audiorouter --check CONFIG` | check | yes | no |
| `audiorouter --list-devices` | list devices | no | no |
| `audiorouter --print-config-path` | print config path | no | no |
| `audiorouter --print-config-path CONFIG` | print config path | no | no |

Mode flags are mutually exclusive:

- `--check`
- `--list-devices`
- `--print-config-path`

Invalid:

```sh
audiorouter --check --list-devices
audiorouter --check --print-config-path
audiorouter --list-devices --print-config-path
```

`--quiet` and `--verbose` are not mode flags and are not mutually exclusive with
each other at the parser level. If both are supplied, `--quiet` takes precedence
and the effective log level is ERROR.

### 3.2 Positional `CONFIG`

`CONFIG` is an optional path to a TOML config file.

Rules:

1. `CONFIG` is a file path, not a profile name.
2. No `.toml` suffix inference is performed.
3. No lookup under `~/.config/audiorouter/` is performed for bare names.
4. If `CONFIG` is relative, resolve it relative to the current working directory.
5. Do not use `std::fs::canonicalize()` for `--print-config-path`, because the file may not exist yet.
6. For `run` and `--check`, reading the file naturally fails if it does not exist.

Examples:

```sh
audiorouter
audiorouter ./vt4.toml
audiorouter --check
audiorouter --check ./vt4.toml
audiorouter --print-config-path
audiorouter --print-config-path ./vt4.toml
```

`CONFIG` is not allowed with `--list-devices`, because that mode does not read configuration:

```sh
audiorouter --list-devices ./vt4.toml
```

must fail with a clear error.

## 4. Configuration path resolution

Resolved config path rules:

1. If positional `CONFIG` is passed, use that path.
   - If relative, convert it to an absolute path by joining it with the current working directory.
   - Do not require the path to exist when only printing it.
2. Else if `XDG_CONFIG_HOME` is set and non-empty, use:

   ```text
   $XDG_CONFIG_HOME/audiorouter/config.toml
   ```

3. Else use:

   ```text
   ~/.config/audiorouter/config.toml
   ```

The program never creates or modifies the config file in v0.1. Users create and edit the TOML file themselves.

## 5. Mode behavior

### 5.1 Default run mode

Invocation:

```sh
audiorouter [CONFIG]
```

Behavior:

1. Resolve config path.
2. Read config. If the file does not exist, print a clear error including the resolved path and a hint such as `Run 'audiorouter --print-config-path' to see the expected location.`
3. Validate config exactly as `--check` does.
4. Open required input streams and output streams.
5. Route/mix audio until SIGINT (Ctrl-C) or a fatal audio error.
6. On SIGINT, set running flag to false; wait for the main loop to exit; drop streams; exit 0.
   SIGTERM is not handled in v0.1; the OS will deliver the default termination.
7. On a fatal stream error, set running flag to false, exit 2.
8. Print a short startup summary unless `--quiet` is set:
   - config path
   - engine sample rate
   - buffer size
   - devices opened as input/output
   - route count

Exit codes:

- `0`: clean Ctrl-C or normal shutdown.
- `1`: config validation failed.
- `2`: runtime/audio device error.

On a fatal stream error during run (CPAL error callback that is unrecoverable):

- Log the error to stderr.
- Signal the main loop to stop (set the running flag to false).
- Allow streams to be dropped and exit with code `2`.
- Do not panic inside the callback.

### 5.2 Check mode

Invocation:

```sh
audiorouter --check [CONFIG]
audiorouter -n [CONFIG]
```

Behavior:

1. Resolve config path.
2. Read and parse config (TOML parse + struct deserialization in one step).
3. Run pure config validation (no CPAL).
4. Resolve device names via CPAL.
5. Validate channel counts and sample-rate support when CPAL reports enough information.
6. Print either a success summary or all discovered validation errors.
7. Do not open long-running audio streams.

Exit codes:

- `0`: valid config.
- `1`: config validation failed.
- `2`: unexpected I/O/runtime/device-query error.

### 5.3 List-devices mode

Invocation:

```sh
audiorouter --list-devices
audiorouter -l
```

Behavior:

- Do not read config.
- Show input devices and output devices separately.
- For each device, show:
  - display name
  - max input channels if available
  - max output channels if available
  - supported sample rates when available
  - default input/output marker if available

Human-readable output is enough for v0.1. JSON output is not required.

Exit codes:

- `0`: success.
- `2`: device-query error.

### 5.4 Print-config-path mode

Invocation:

```sh
audiorouter --print-config-path
audiorouter --print-config-path CONFIG
```

Behavior:

- Do not read config.
- Print exactly one path to stdout.
- If `CONFIG` is supplied, print the resolved absolute path for that argument.
- If `CONFIG` is omitted, print the resolved default config path.
- Do not create directories or files.

Exit codes:

- `0`: success.
- `2`: path resolution error, such as no discoverable home directory for the default path.

## 6. TOML schema

### 6.1 Full example

```toml
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "vt4"
device = "VT-4"

[[devices]]
name = "mic"
device = "MacBook Pro Microphone"

[[devices]]
name = "blackhole"
device = "BlackHole 2ch"
limiter = true

[[devices]]
name = "speaker"
device = "MacBook Pro Speakers"

[[routes]]
from = "vt4"
to = "blackhole"
from_channels = [3, 4]
to_channels = [1, 2]
gain_db = 0.0

[[routes]]
from = "mic"
to = "blackhole"
from_channels = [1, 1]
to_channels = [1, 2]
gain_db = -8.0

[[routes]]
from = "vt4"
to = "speaker"
from_channels = [3, 4]
to_channels = [1, 2]
gain_db = -12.0
```

### 6.2 `[engine]`

Optional table. If omitted, the engine uses the defaults below.

| Field | Type | Required | Default | Description |
|---|---:|---:|---:|---|
| `sample_rate` | integer | no | `48000` | Engine sample rate in Hz. v0.1 requires all opened devices to support this rate. |
| `buffer_size` | integer | no | `256` | Desired audio buffer size in frames. |

Validation:

- `sample_rate` must be positive.
- Recommended sample rates are `44100` and `48000`, but do not hard-code only these if the device reports others.
- `buffer_size` must be positive.
- Recommended `buffer_size` range is `64..=2048`.
- v0.1 may warn outside the recommended range instead of failing.

### 6.3 `[[devices]]`

Array of named device definitions. Entries may be omitted for devices referenced directly by routes; missing route endpoints are treated as implicit devices with `name = device = <route string>` and `limiter = false`.

| Field | Type | Required | Default | Description |
|---|---:|---:|---:|---|
| `name` | string | no | value of `device` | Stable config-local alias used by routes. |
| `device` | string | yes | none | Human-readable CoreAudio device name to match. |
| `limiter` | bool | no | `false` | Applies only when this device is used as an output. Enables simple output limiter. |

No `type` field exists in v0.1. A device's role is inferred from routes:

- Appears in `route.from` => opened as input.
- Appears in `route.to` => opened as output.
- Appears in both => opened as both input and output if the underlying device supports duplex use.

Validation:

- `name` must be non-empty.
- `name` must be unique.
- `device` must be non-empty.
- A defined device not used by any route should produce a warning, not an error.
- Matching by exact device name is required for v0.1.
- Optional future improvement: unique case-insensitive substring matching. Do not implement substring matching unless ambiguity handling is also implemented.

### 6.4 `[[routes]]`

Array of channel mappings.

| Field | Type | Required | Default | Description |
|---|---:|---:|---:|---|
| `from` | string | yes | none | Source device alias. |
| `to` | string | yes | none | Destination device alias. |
| `from_channels` | array of positive integers | yes | none | Source physical input channels, 1-based. |
| `to_channels` | array of positive integers | yes | none | Destination physical output channels, 1-based. |
| `gain_db` | float | no | `0.0` | Gain applied to this route before mixing. |
| `mute` | bool | no | `false` | If true, route contributes silence. Optional in v0.1; easy to support. |

Channel arrays use **1-based physical channel numbers**. There is no hidden internal channel renumbering in the config.

Examples:

```toml
from_channels = [3, 4]
to_channels = [1, 2]
```

means:

```text
source physical ch3 -> output physical ch1
source physical ch4 -> output physical ch2
```

Mono-to-stereo is expressed by repeating the source channel:

```toml
from_channels = [1, 1]
to_channels = [1, 2]
```

means:

```text
source physical ch1 -> output physical ch1
source physical ch1 -> output physical ch2
```

Validation:

- If `from` does not refer to an explicit `devices.name`, it is treated as an implicit device with `name = device = from`.
- If `to` does not refer to an explicit `devices.name`, it is treated as an implicit device with `name = device = to`.
- `from_channels` and `to_channels` must have the same length.
- Both arrays must be non-empty.
- Every channel number must be `>= 1`.
- `from` and `to` may be the same only if the implementation can safely avoid feedback; for v0.1, reject same-device routes with a clear error.
- `gain_db` should be finite. Reject NaN and infinity.
- If `mute = true`, still validate all device/channel references.

## 7. Device role inference

After parsing config:

1. Build a map of `devices.name -> DeviceConfig`.
2. Initialize role flags for every device:
   - `needs_input = false`
   - `needs_output = false`
3. For each route:
   - set `route.from.needs_input = true`
   - set `route.to.needs_output = true`
4. A device may have both flags.
5. A device with neither flag is unused and should warn.

Required channel counts are derived from routes:

- For each input device, required input channel count is the max of all `from_channels` where it appears as `from`.
- For each output device, required output channel count is the max of all `to_channels` where it appears as `to`.

## 8. Audio processing model

### 8.1 Internal sample format

Use `f32` internally.

If CPAL provides other formats, convert to/from `f32` at the stream boundary.

Config channel numbers are **1-based physical channel indices**. For the mapping
`physical channel N -> interleaved index N-1` to be correct, each stream must be
opened with the device's full physical channel count, **not** the route's
`required_*_channels`. Opening a 4-channel device with only 2 channels would make
channels 3 and 4 unaddressable and silently misroute audio. Choose the supported
config whose channel count exposes every channel the routes reference (typically
the device's max channel count), then index into it.

### 8.2 Gain conversion

Route gain in dB converts to linear gain as:

```text
linear_gain = 10^(gain_db / 20)
```

For muted routes, effective gain is `0.0`.

### 8.3 Buffer topology and fan-out

A single input device may feed multiple output devices (goal 4), and the headline
example in section 6.1 routes `vt4` to both `blackhole` and `speaker`. This
constrains the shared-buffer design:

- A standard SPSC ring buffer has **one** consumer and reads are **destructive**.
  A single ring buffer per input device therefore **cannot** fan out to multiple
  outputs: whichever output callback reads first consumes the frames.

v0.1 must use one of these fan-out-safe designs:

1. **Per-route ring buffer (preferred).** Allocate one SPSC ring buffer per
   route (per `from -> to` edge). The input callback for device `D` is the single
   producer for every route where `D` is `from`, and writes the source device's
   **full physical interleaved frames** into each of those routes' ring buffers.
   The output callback for device `O` is the single consumer of every route ring
   buffer where `O` is `to`. Each route buffer therefore carries all of
   `route.from`'s physical channels, so the mixer can index by 1-based physical
   channel number (consistent with 8.1; no internal channel renumbering). This
   keeps SPSC discipline, supports fan-out, and is real-time friendly.
2. **Non-destructive shared latest-buffer (acceptable fallback).** Each input
   callback writes its latest block into an `Arc<Mutex<Vec<f32>>>`; each output
   callback locks and *copies* (non-destructive) the frames it needs. Fan-out
   works naturally because reads do not consume. Document that this is not
   sample-accurate under load and may briefly block on the mutex.

Do not use a single ring buffer per input device with multiple consumers.

### 8.4 Mixing rule

For each output callback buffer:

1. Clear the output buffer to silence.
2. Find all routes targeting that output device.
3. For each route:
   - Read up to `frame_count` source frames for `route.from` from that route's
     buffer (per the topology in 8.3).
   - For each mapping pair `(from_channel, to_channel)`:
     - Convert channel numbers from 1-based config to 0-based buffer indices.
     - Multiply source sample by route gain.
     - Add into destination output sample.
4. If output device has `limiter = true`, apply limiter to final mixed output.
5. Write output buffer.

Multiple routes writing to the same output channel are summed.

### 8.5 Missing, late, or excess input data

Independent input and output streams run on independent device clocks and drift
over time, so a steady stream of underruns and overruns is expected, not
exceptional.

If a route buffer has **insufficient** frames for an output callback (underrun):

- Fill the missing portion with silence.
- Do not panic.
- Count underruns for diagnostics if easy.

If a route buffer is **full** when the input callback tries to write (overrun):

- Drop the oldest frames (or skip the write) so the producer never blocks.
- Do not panic or grow the buffer unboundedly.
- Count overruns for diagnostics if easy.

### 8.6 Limiter v0.1

A simple limiter is acceptable:

```text
sample = clamp(sample, -1.0, 1.0)
```

This is technically a hard clipper, not a production limiter. Name the implementation clearly as `hard_limit_sample` (per-sample) and `hard_limit_buffer` (slice variant), and document that v0.1 uses hard clipping.

A better look-ahead limiter can be added later without changing the config schema.

### 8.7 Sample rate handling

v0.1 should fail fast if any required device cannot run at `engine.sample_rate`.

Do not silently resample in v0.1 unless a proper resampler is implemented and tested.

### 8.8 Buffer size handling

Use `engine.buffer_size` as the requested stream buffer size when CPAL supports it.

If the host/device refuses the requested fixed buffer size, either:

- fall back to the device default and print a warning, or
- fail with a clear error.

For v0.1, prefer fail-fast during `--check`/run if validation can detect unsupported buffer size; otherwise report the actual stream config during run.

### 8.9 Ring buffer sizing

With the per-route ring buffer design (8.3, option 1), each route owns one ring
buffer carrying the full physical interleaved frames of its `route.from` device.

Ring buffer capacity for v0.1: at least `engine.buffer_size * channels * 4`
**f32 elements**, where `channels` is the physical channel count of the
`route.from` device (the channel count the input stream is opened with, per 8.1),
**not** `from_channels.len()`. The `* 4` gives roughly four output-period headroom
against scheduling jitter and clock drift. Capacity is in samples (f32 elements),
not frames — do not forget the `channels` factor.

If the non-destructive latest-buffer design (8.3, option 2) is used instead, size
each input's shared buffer to one block: `engine.buffer_size * channels` f32
elements, and document the not-sample-accurate limitation. See section 9 for the
module layout.

## 9. Runtime architecture

Suggested modules:

```text
src/main.rs        CLI entrypoint: parse args, dispatch mode, convert errors to exit codes
src/cli.rs         clap option definitions, mode selection, argument validation
src/config.rs      config path resolution, TOML parsing, structs
src/validate.rs    config validation and route/device role inference (no CPAL)
src/devices.rs     CPAL device enumeration and resolution
src/audio.rs       stream startup/shutdown orchestration:
                     - open CPAL input/output streams per device
                     - allocate and wire shared ring buffers
                     - hold stream handles alive for the process lifetime
                     - handle SIGINT/fatal-stream-error shutdown
src/mixer.rs       gain, channel mapping, summing, limiter helpers (no CPAL)
```

Keep logic testable:

- CLI mode selection should be unit-testable without audio devices.
- Config parsing should not touch audio devices.
- Validation should be separable from stream startup.
- Mixer math should be unit-testable without CPAL.

## 10. Error messages

Errors should be specific enough that users can fix config files quickly.

Good examples:

```text
error: CONFIG cannot be used with --list-devices
error: --check, --list-devices, and --print-config-path are mutually exclusive
error: route[1].from references unknown device alias "vt-4"; known devices: vt4, mic, blackhole
error: route[2] maps from_channels length 1 to to_channels length 2; lengths must match. Use from_channels = [1, 1] for mono-to-stereo.
error: device "vt4" is used as input and route requires channel 4, but CoreAudio reports only 2 input channels for "VT-4".
error: output device "blackhole" resolved to "BlackHole 2ch", but route requires output channel 3.
```

Avoid vague errors like:

```text
invalid config
stream failed
```

### 10.1 Exit code mechanism

The modes above distinguish exit code `1` (config/validation failure) from `2`
(runtime/device/I-O error). A bare `fn main() -> anyhow::Result<()>` cannot
express this: it returns `1` for every `Err`. The implementation must map errors
to codes explicitly, for example:

- keep `main` returning a normal `Result` to a thin wrapper, and have the wrapper
  call `std::process::exit(code)` with an explicit code, or
- carry a category on the error (a small enum or an `anyhow` downcast) and select
  the code in one place before exiting.

Whatever the mechanism, the codes in section 5 are the contract; tests or manual
checks should confirm at least the `1` vs `2` distinction for a validation error
versus a missing/unavailable device.

## 11. Testing requirements

v0.1 should include unit tests for non-audio logic:

1. CLI mode selection:
   - no flags => run
   - `--check` => check
   - `--list-devices` => list devices
   - `--print-config-path` => print config path
   - multiple mode flags fail
   - `CONFIG` with `--list-devices` fails
2. Config path resolution with and without `XDG_CONFIG_HOME`.
3. Positional relative `CONFIG` is resolved against current working directory without requiring the file to exist for `--print-config-path`.
4. TOML parsing of the sample config.
5. Duplicate device names fail validation.
6. Unknown route device fails validation.
7. Channel array length mismatch fails validation.
8. Zero channel number fails validation.
9. Same-device route (`from == to`) fails validation.
10. Non-finite `gain_db` (NaN/inf) fails validation.
11. Device role inference works:
    - from-only => input
    - to-only => output
    - both => input and output
12. Required channel counts are computed from routes.
13. dB to linear gain conversion.
14. Mixer channel mapping:
    - stereo passthrough
    - mono-to-stereo via `[1, 1] -> [1, 2]`
    - multiple routes summed into one output
    - one input fanned out to two outputs (both receive the source)
    - hard limiter clamps output

Manual/ad-hoc verification on macOS:

1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `cargo test`
4. `cargo run -- --help`
5. `cargo run -- --list-devices`
6. `cargo run -- --print-config-path`
7. Create a temporary TOML config manually using the sample in this spec.
8. `cargo run -- --check /tmp/audiorouter-test.toml`
9. With BlackHole installed and a valid input device configured:
   - `cargo run -- /tmp/audiorouter-real.toml`
   - confirm receiving app sees audio on BlackHole.

## 12. Dependency guidance

Recommended crates:

```toml
anyhow = ">=1,<2"
clap = { version = ">=4,<5", features = ["derive"] }
serde = { version = ">=1,<2", features = ["derive"] }
toml = ">=0.8,<0.9"
cpal = ">=0.15,<0.16"
ctrlc = ">=3,<4"
dirs = ">=5,<7"
tracing = ">=0.1,<0.2"
tracing-subscriber = ">=0.3,<0.4"
```

Do not add a resampling dependency in v0.1 unless resampling is actually implemented.

## 13. Sample config

Users create the config file manually. v0.1 does not provide an `init` command or `--example-config`; the sample below exists only in this specification and can be copied into the default config path or another TOML file path passed as positional `CONFIG`:

```toml
[engine]
sample_rate = 48000
buffer_size = 256

# Run `audiorouter --list-devices` to find the exact device names on your system.

[[devices]]
name = "source"
device = "Your Input Device"

[[devices]]
name = "blackhole"
device = "BlackHole 2ch"
limiter = true

# Stereo route: source ch1/ch2 -> BlackHole ch1/ch2
[[routes]]
from = "source"
to = "blackhole"
from_channels = [1, 2]
to_channels = [1, 2]
gain_db = 0.0
```

## 14. v0.1 acceptance criteria

The v0.1 implementation is acceptable when:

- `cargo fmt --check` passes.
- `cargo clippy -- -D warnings` passes.
- `cargo test` passes.
- `audiorouter --help` documents `audiorouter [OPTIONS] [CONFIG]` with no subcommands.
- `audiorouter --list-devices` lists CoreAudio devices on macOS.
- `audiorouter --print-config-path` prints the default config path.
- `audiorouter --check CONFIG` catches invalid aliases and invalid channel mappings.
- `audiorouter CONFIG` can route at least one stereo input to BlackHole 2ch on a machine where BlackHole is installed.
- Ctrl-C exits without panic and with exit code 0.
- Running with a missing config file produces a clear error message including the resolved path.
- `audiorouter --quiet CONFIG` suppresses the startup summary.
- Config syntax remains only `devices` + `routes` for routing concepts.
