# audiorouter v0.1 Implementation Plan

This plan is written for a low-cost implementation model. Follow the steps in order. Do not skip validation or tests. Keep v0.1 small: only `devices` and `routes` as routing concepts, and use a subcommand-free CLI.

## Current desired v0.1 behavior

Build a Rust CLI that:

1. Reads TOML config from:
   - positional `CONFIG`, if supplied;
   - else `$XDG_CONFIG_HOME/audiorouter/config.toml`;
   - else `~/.config/audiorouter/config.toml`.
2. Provides no subcommands.
3. Uses mode flags:
   - default mode: run audio routing;
   - `--check` / `-n`: validate config and devices, then exit;
   - `--list-devices` / `-l`: list devices, then exit without reading config;
   - `--print-config-path`: print resolved config path, then exit without reading config.
4. Uses config sections:
   - `[engine]`
   - `[[devices]]`
   - `[[routes]]`
5. Infers device input/output roles from route direction.
6. Routes and mixes audio from physical input channels to physical output channels.
7. Uses BlackHole or another output device as the sink.

Read `SPEC.md` before coding. If this plan conflicts with `SPEC.md`, treat `SPEC.md` as authoritative.

---

## Phase 0: Project setup

### 0.1 Update `Cargo.toml`

Add dependencies with bounded versions:

```toml
[dependencies]
anyhow = ">=1,<2"
clap = { version = ">=4,<5", features = ["derive"] }
serde = { version = ">=1,<2", features = ["derive"] }
toml = ">=0.8,<0.9"
cpal = ">=0.15,<0.16"
ringbuf = ">=0.4,<0.5"
ctrlc = ">=3,<4"
dirs = ">=5,<7"
tracing = ">=0.1,<0.2"
tracing-subscriber = ">=0.3,<0.4"
```

Do not add `rubato` or another resampler in v0.1.

`ringbuf` is the preferred shared-memory ring buffer between input callbacks and output callbacks. If `ringbuf` 0.4 API proves too complex, you may substitute a simpler `Arc<Mutex<Vec<f32>>>` latest-buffer approach for an initial working version, but document the limitation clearly.

### 0.2 Create module files

Create these files:

```text
src/cli.rs
src/config.rs
src/validate.rs
src/devices.rs
src/audio.rs
src/mixer.rs
```

Keep `src/main.rs` very small.

Suggested `main.rs` responsibility:

- initialize tracing/logging:
  - `--quiet`: ERROR level only
  - default (no flag): WARN level
  - `-v`: DEBUG level
  - `-vv` or more: TRACE level
- parse CLI args
- dispatch selected mode
- convert errors into process exit code (1 for validation, 2 for runtime/device)

---

## Phase 1: CLI skeleton

### 1.1 Implement clap structures in `src/cli.rs`

There are no subcommands.

Define:

```rust
use std::path::PathBuf;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Cli {
    /// TOML configuration file to read.
    ///
    /// If omitted, audiorouter reads the default XDG config path.
    pub config: Option<PathBuf>,

    /// Validate configuration and device availability, then exit.
    #[arg(short = 'n', long)]
    pub check: bool,

    /// List available audio input/output devices, then exit.
    /// Does not read CONFIG.
    #[arg(short = 'l', long)]
    pub list_devices: bool,

    /// Print the resolved configuration path, then exit.
    #[arg(long)]
    pub print_config_path: bool,

    /// Suppress non-error output.
    #[arg(short, long)]
    pub quiet: bool,

    /// Print extra diagnostics. Repeat for more detail.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Check,
    ListDevices,
    PrintConfigPath,
}
```

### 1.2 Implement mode selection

Implement a method or function:

```rust
impl Cli {
    pub fn mode(&self) -> anyhow::Result<Mode> {
        let selected = [self.check, self.list_devices, self.print_config_path]
            .into_iter()
            .filter(|selected| *selected)
            .count();

        if selected > 1 {
            anyhow::bail!(
                "--check, --list-devices, and --print-config-path are mutually exclusive"
            );
        }

        let mode = if self.check {
            Mode::Check
        } else if self.list_devices {
            Mode::ListDevices
        } else if self.print_config_path {
            Mode::PrintConfigPath
        } else {
            Mode::Run
        };

        if self.config.is_some() && matches!(mode, Mode::ListDevices) {
            anyhow::bail!("CONFIG cannot be used with --list-devices");
        }

        Ok(mode)
    }
}
```

`CONFIG` is allowed with:

- `Run`
- `Check`
- `PrintConfigPath`

`CONFIG` is not allowed with:

- `ListDevices`

### 1.3 Main dispatch

`main.rs` should dispatch:

```rust
match cli.mode()? {
    Mode::Run => ...,
    Mode::Check => ...,
    Mode::ListDevices => ...,
    Mode::PrintConfigPath => ...,
}
```

For now, modes may print placeholder messages until later phases are implemented.

Exit codes: `1` means config/validation failure, `2` means runtime/device error
(see SPEC.md section 5 and 10.1). A plain `fn main() -> anyhow::Result<()>` always
exits `1` on error, so do not rely on it for the `1` vs `2` split. Instead have
`main` compute the code and call `std::process::exit(code)` explicitly — e.g. run
the real logic in `fn run() -> Result<()>`, then in `main` match the error
category to `1` or `2`.

### 1.4 Verify

Run:

```sh
cargo fmt
cargo run -- --help
cargo run -- --check --help
cargo run -- --list-devices --help
cargo run -- --print-config-path --help
```

Note: `--check --help` just prints global help because there are no subcommands. This is fine.

Confirm help shows a usage shaped like:

```text
audiorouter [OPTIONS] [CONFIG]
```

### 1.5 CLI tests

Add tests for mode selection:

- no flags => `Mode::Run`
- `--check` => `Mode::Check`
- `--list-devices` => `Mode::ListDevices`
- `--print-config-path` => `Mode::PrintConfigPath`
- `--check --list-devices` fails
- `--check --print-config-path` fails
- `--list-devices --print-config-path` fails
- `CONFIG` with `--list-devices` fails
- `CONFIG` with `--check` is allowed
- `CONFIG` with `--print-config-path` is allowed

---

## Phase 2: Config structs and path resolution

### 2.1 Implement config structs in `src/config.rs`

Use serde deserialize.

Required structs:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub engine: EngineConfig,
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EngineConfig {
    pub sample_rate: u32,
    pub buffer_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub device: String,
    #[serde(default)]
    pub limiter: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    pub from: String,
    pub to: String,
    pub from_channels: Vec<usize>,
    pub to_channels: Vec<usize>,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub mute: bool,
}
```

Channel numbers are stored as `usize` but represent **1-based physical channel numbers**.

### 2.2 Config path resolution

Implement:

```rust
pub fn default_config_path() -> anyhow::Result<PathBuf>
pub fn resolve_config_path(config_arg: Option<&Path>) -> anyhow::Result<PathBuf>
```

Rules:

1. If `config_arg` is Some:
   - If absolute, return it as a `PathBuf`.
   - If relative, return `std::env::current_dir()?.join(config_arg)`.
   - Do not call `canonicalize()` here, because `--print-config-path` must work even if the file does not exist.
2. Else if `XDG_CONFIG_HOME` is set and non-empty, return `$XDG_CONFIG_HOME/audiorouter/config.toml`.
3. Else return `dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?.join(".config/audiorouter/config.toml")`.

Note: `dirs::home_dir()` returns `Option<PathBuf>` in all current versions of the `dirs` crate. Do not use any deprecated `home_dir` function from the standard library.

### 2.3 Read config

Implement:

```rust
pub fn read_config(path: &Path) -> anyhow::Result<Config>
```

Behavior:

- Read file as UTF-8 text.
- Parse as TOML.
- Include path in error context.
- If the file does not exist, return an error whose message includes:
  - the resolved path
  - the hint `Run 'audiorouter --print-config-path' to see the expected location.`

### 2.4 No config-writing or config-template command

Do not implement config generation in v0.1.

Specifically:

- No `init` command.
- No `--example-config` option.
- No command or option that writes config files.
- No command or option that prints a config template.

Users create TOML files manually, using `SPEC.md` section 13 as a copyable sample.

### 2.5 Tests

Add tests in `config.rs`:

- parses the sample config from `SPEC.md` copied into a test string
- `XDG_CONFIG_HOME` path resolution
- fallback home path if practical
- positional absolute `CONFIG` path is returned unchanged
- positional relative `CONFIG` path is joined with current working directory
- missing config path produces a clear error from `read_config`

Use `tempfile` only if you add it as a dev-dependency. Otherwise write tests that do not require filesystem temp dirs except via standard library.

---

## Phase 3: Validation and role inference

### 3.1 Create validation types in `src/validate.rs`

Define a validated plan that is independent of CPAL streams.

Suggested structs:

```rust
pub struct ValidatedConfig {
    pub config: Config,
    pub devices: Vec<ResolvedDeviceRole>,
    pub routes: Vec<ValidatedRoute>,
    pub warnings: Vec<String>,
}

pub struct ResolvedDeviceRole {
    pub name: String,
    pub device: String,
    pub limiter: bool,
    pub needs_input: bool,
    pub needs_output: bool,
    pub required_input_channels: usize,
    pub required_output_channels: usize,
}

pub struct ValidatedRoute {
    pub from: String,
    pub to: String,
    pub from_channels: Vec<usize>,
    pub to_channels: Vec<usize>,
    pub gain_db: f32,
    pub mute: bool,
}
```

### 3.2 Implement pure config validation

Implement:

```rust
pub fn validate_config(config: Config) -> Result<ValidatedConfig, Vec<String>>
```

This function should not touch CPAL. It only validates internal config consistency.

Validation rules:

1. `engine.sample_rate > 0`.
2. `engine.buffer_size > 0`.
3. At least one device.
4. At least one route.
5. Device `name` (alias) non-empty.
6. Device `device` (CoreAudio name) non-empty.
7. Device `name` values unique.
8. Every route `from` exists.
9. Every route `to` exists.
10. Reject `from == to` in v0.1.
11. `from_channels` non-empty.
12. `to_channels` non-empty.
13. `from_channels.len() == to_channels.len()`.
14. Every channel number is >= 1.
15. `gain_db.is_finite()`.
16. Infer roles and required channel counts.
17. Warn for unused devices.

Collect as many errors as possible rather than returning after the first error.

### 3.3 Error wording

Use explicit messages. Include route index and device alias where possible.

Examples:

```text
route[0].from references unknown device alias "foo"
route[1] has from_channels length 1 but to_channels length 2
route[2].from_channels contains invalid channel 0; channels are 1-based
```

### 3.4 Check mode implementation

Implement check mode:

1. Resolve path from positional `CONFIG` or default path.
2. Read config.
3. Run pure validation.
4. If pure validation fails, print all errors and exit 1.
5. Resolve actual audio devices and validate channel counts in Phase 4.
6. For now, print pure validation success if Phase 4 is not done.

### 3.5 Tests

Add unit tests for:

- duplicate device names fail
- unknown route `from` fails
- unknown route `to` fails
- mismatched channel lengths fail
- zero channel fails
- `from == to` fails
- non-finite `gain_db` (NaN/inf) fails
- role inference from route direction
- required channel count inference
- unused device warning

Run:

```sh
cargo test
```

---

## Phase 4: Device enumeration and resolution

### 4.1 Implement `src/devices.rs`

Use CPAL.

Functions:

```rust
pub fn print_devices() -> anyhow::Result<()>
pub fn resolve_devices(plan: &ValidatedConfig) -> anyhow::Result<ResolvedAudioDevices>
```

You can adjust type names, but keep responsibilities clear.

### 4.2 Device listing

`audiorouter --list-devices` should:

1. Get default host: `cpal::default_host()`.
2. Print input devices.
3. Print output devices.
4. For each, print device name and supported configs if available.
5. Do not read config.

CPAL APIs to inspect:

- `host.input_devices()`
- `host.output_devices()`
- `device.name()`
- `device.supported_input_configs()`
- `device.supported_output_configs()`
- `host.default_input_device()`
- `host.default_output_device()`

Make output robust: if supported configs cannot be queried, print a warning for that device and continue.

### 4.3 Exact device matching

v0.1 matching rule:

- A config `device = "..."` must exactly match `cpal::Device::name()`.

For a device used as input, search input devices.
For a device used as output, search output devices.
For a device used as both, search both lists.

If not found, error:

```text
device alias "vt4" uses CoreAudio device "VT-4" as input, but no matching input device was found. Run `audiorouter --list-devices`.
```

### 4.4 Channel-count validation against actual devices

For each resolved input:

- Determine maximum supported input channels across supported input configs.
- Ensure `required_input_channels <= max_input_channels`.

For each resolved output:

- Determine maximum supported output channels across supported output configs.
- Ensure `required_output_channels <= max_output_channels`.

If supported configs cannot be queried, allow run mode to attempt opening but print warning during `--check`.

### 4.5 Sample-rate validation

For each required input/output device:

- Verify at least one supported config range includes `engine.sample_rate`.
- If unavailable, warn or defer to stream opening.
- Prefer clear error if configs are available and sample rate is unsupported.

### 4.6 Integrate with check mode

`audiorouter --check [CONFIG]` should run:

1. TOML parse.
2. Pure validation.
3. CPAL device resolution.
4. Channel/sample-rate validation.

Print warnings, then success summary:

```text
Config OK: 4 devices, 3 routes, sample_rate=48000, buffer_size=256
Inputs: vt4 -> VT-4, mic -> MacBook Pro Microphone
Outputs: blackhole -> BlackHole 2ch, speaker -> MacBook Pro Speakers
```

---

## Phase 5: Mixer math

### 5.1 Implement `src/mixer.rs`

Pure functions first. Do not involve CPAL here.

Required functions:

```rust
pub fn db_to_linear(db: f32) -> f32        // 10^(db/20)
pub fn hard_limit_sample(x: f32) -> f32    // clamp(x, -1.0, 1.0)
pub fn hard_limit_buffer(buf: &mut [f32])  // applies hard_limit_sample to every element
```

Then implement a small channel mapping helper.

Suggested function shape:

```rust
pub fn mix_route_interleaved(
    input: &[f32],
    input_channels: usize,
    output: &mut [f32],
    output_channels: usize,
    from_channels_1based: &[usize],
    to_channels_1based: &[usize],
    gain: f32,
)
```

Assumptions:

- Buffers are interleaved.
- Frame count is `min(input.len() / input_channels, output.len() / output_channels)`.
- Channel numbers are 1-based.
- Caller already validated channel numbers.

### 5.2 Tests

Test:

1. `db_to_linear(0.0) == 1.0` approximately.
2. `db_to_linear(-6.0)` approximately `0.501`.
3. hard limiter clamps above 1 and below -1.
4. stereo passthrough:
   - input L/R -> output L/R.
5. mono-to-stereo:
   - input ch1 repeated to output ch1/ch2.
6. summing:
   - call mix twice into same output and confirm values add.
7. fan-out:
   - mix one input into two separate output buffers; both receive the source
     (a non-destructive read; confirms 1-input -> N-output works).

Do this before writing the real audio engine.

---

## Phase 6: Audio engine v0.1

This is the hardest part. Keep it minimal.

### 6.1 Initial implementation strategy

All audio engine code lives in `src/audio.rs`. `src/main.rs` calls into it; `src/mixer.rs` is called from within it.

Read SPEC.md section 8.3 first. The key constraint: one input can fan out to
multiple outputs (the sample config sends `vt4` to both `blackhole` and
`speaker`), and an SPSC ring buffer has a single, destructive consumer. So a
single ring buffer **per input device** does not work — use **one ring buffer
per route**.

Preferred approach — one `ringbuf` SPSC ring per route:

```rust
use ringbuf::{HeapRb, traits::*};

// channels = physical channel count of route.from's input stream (NOT
// from_channels.len()). The route buffer carries the source device's full
// physical frames so the mixer can index by 1-based physical channel.
// Capacity is in f32 elements (samples), so multiply by channels.
let capacity = engine.buffer_size as usize * channels * 4;
let rb = HeapRb::<f32>::new(capacity);
let (prod, cons) = rb.split();
// prod: written by the input callback of route.from (single producer)
// cons: read by the output callback of route.to   (single consumer)
```

- The input callback for device `D` writes its full physical interleaved frames
  into the producer of **every** route where `D` is `from`.
- The output callback for device `O` reads from the consumer of **every** route
  where `O` is `to`, then mixes (see `src/mixer.rs`).
- On overrun (producer full), drop oldest/skip; never block the callback.
- On underrun (consumer empty), fill with silence.

Fallback — if the `ringbuf` 0.4 split/producer API proves too fiddly, use a
non-destructive shared latest-buffer keyed by input device:

```rust
struct LatestInputBuffer {
    channels: usize,
    frames: Vec<f32>,  // interleaved, len = buffer_size * channels
}
// Arc<Mutex<LatestInputBuffer>> per input device.
// Output callbacks lock and COPY (non-destructive) so fan-out still works.
```

If using the fallback, document clearly that v0.1 uses latest-buffer mixing and may not be sample-accurate under load. The fallback must still not block for long inside callbacks.

### 6.2 Stream config

For each input device (open it **once**, even if it is `from` in many routes):

- Open one input stream.
- Use `engine.sample_rate`.
- Open with the device's **full physical channel count** (the channel count of
  the chosen supported config), not `required_input_channels`. Physical channel
  `N` maps to interleaved index `N-1` only when all channels are present; see
  SPEC.md 8.1. `required_input_channels` is only the minimum the device must
  expose, used during validation.
- Convert incoming samples to f32.
- Write frames into each outgoing route's buffer.

For each output device (open it **once**, even if many routes target it):

- Open one output stream.
- Use `engine.sample_rate`.
- Open with the device's full physical channel count, same reasoning as inputs.
- In callback:
  - clear output to zero
  - collect routes targeting this output
  - read needed source data
  - call mixer mapping for each route
  - apply limiter if configured

Important: open each physical input and output device only once, then share its
stream across all routes that reference it.

### 6.3 CPAL sample formats

CPAL streams may use `f32`, `i16`, `u16`, etc.

Implement helpers to convert input samples to f32 and f32 output to the output sample type.

CPAL examples usually use match on `SampleFormat` and generic callback functions. Follow that pattern.

### 6.4 Shutdown

Use `ctrlc` crate:

- Create an `Arc<AtomicBool>` running flag (also used by fatal-error path in 6.5).
- Ctrl-C (SIGINT) handler sets running to false.
- Run mode starts streams and then sleeps in a loop while running is true.
- Streams are dropped at the end of the `run_audio` function (natural drop when the `Vec<Stream>` goes out of scope).
- SIGTERM is not handled in v0.1; the OS default applies.

Pseudo-code:

```rust
// streams: Vec<cpal::Stream> — keep alive here
let running = Arc::new(AtomicBool::new(true));
let r = running.clone();
ctrlc::set_handler(move || { r.store(false, Ordering::SeqCst); })?;

while running.load(Ordering::SeqCst) {
    std::thread::sleep(Duration::from_millis(100));
}
// streams dropped here -> CPAL stops callbacks
```

### 6.5 Error callbacks

CPAL stream error callbacks should print errors to stderr. For fatal/unrecoverable errors:

- Store a clone of the `running` `Arc<AtomicBool>` in the error closure.
- Set running to false so the main loop exits.
- Store an `Arc<AtomicBool> fatal_error` flag; if true, main returns exit code 2.

Do not panic inside callbacks.

### 6.6 Startup summary

Before playing streams, print unless `--quiet` is set:

```text
Using config: ...
Engine: 48000 Hz, buffer_size=256
Inputs:
  vt4 -> VT-4, required channels: 4
Outputs:
  blackhole -> BlackHole 2ch, required channels: 2, limiter: true
Routes:
  vt4 [3,4] -> blackhole [1,2], gain=0.0 dB
```

---

## Phase 7: Verification

Run after each meaningful phase:

```sh
cargo fmt --check
cargo test
```

Before considering v0.1 complete, run:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo run -- --help
cargo run -- --list-devices
cargo run -- --print-config-path
```

Then test config commands with a manually created TOML file:

```sh
cat > /tmp/audiorouter-test.toml <<'EOF'
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "source"
device = "Your Input Device"

[[devices]]
name = "blackhole"
device = "BlackHole 2ch"
limiter = true

[[routes]]
from = "source"
to = "blackhole"
from_channels = [1, 2]
to_channels = [1, 2]
gain_db = 0.0
EOF

cargo run -- --check /tmp/audiorouter-test.toml
```

The placeholder device names may fail device resolution until edited. That is acceptable. Also test a syntactically valid config with actual device names from `cargo run -- --list-devices`.

For real routing verification on macOS:

1. Install or confirm BlackHole 2ch exists.
2. Run `cargo run -- --list-devices` and copy exact names.
3. Create a real config, e.g. `/tmp/audiorouter-real.toml`.
4. Run:

   ```sh
   cargo run -- --check /tmp/audiorouter-real.toml
   cargo run -- /tmp/audiorouter-real.toml
   ```

5. Select BlackHole in a receiving app or observe levels in Audio MIDI Setup/OBS/DAW.
6. Press Ctrl-C and confirm clean exit.

---

## Recommended implementation order checklist

Use this as a task list.

- [ ] Add dependencies to `Cargo.toml` (including `ringbuf`).
- [ ] Create module files.
- [ ] Implement CLI parser and mode selection.
- [ ] Implement CLI mode validation.
- [ ] Implement config path resolution for positional `CONFIG`.
- [ ] Implement config structs and TOML parsing.
- [ ] Add CLI/config parsing/path tests.
- [ ] Implement pure validation and role inference.
- [ ] Add validation tests.
- [ ] Implement `--list-devices`.
- [ ] Implement exact device resolution.
- [ ] Integrate device/channel/sample-rate validation into `--check`.
- [ ] Implement `--print-config-path`.
- [ ] Implement mixer math.
- [ ] Add mixer tests.
- [ ] Implement minimal audio engine for default run mode.
- [ ] Add Ctrl-C shutdown.
- [ ] Run fmt/clippy/tests.
- [ ] Manually verify `--list-devices`, `--print-config-path`, and `--check` with a hand-written config.
- [ ] Manually verify real routing to BlackHole if available.

---

## Important design constraints

1. Do not implement subcommands in v0.1.
   - No `audiorouter run`.
   - No `audiorouter check`.
   - No `audiorouter devices`.
   - Default run is simply `audiorouter [CONFIG]`.
2. Do not implement `--config` in v0.1.
   - Config selection is positional `CONFIG` only.
3. Do not introduce `type = "input" | "output"` in `[[devices]]`.
   - Role comes from route direction.
4. Do not introduce `buses` in v0.1.
5. Do not introduce `sends` in v0.1.
6. Do not renumber channels internally in config semantics.
   - Config channel numbers are always 1-based physical channel numbers.
7. Do not silently resample in v0.1.
8. Do not open the same physical device once per route.
   - Open one input stream per input device and one output stream per output device.
   - Mix all routes targeting an output into that single output stream.
   - One input may fan out to several outputs; use one ring buffer per route (not
     per input device) so the single-consumer SPSC discipline still holds.
9. Do not panic in audio callbacks.
10. Keep mixer and validation logic unit-testable without audio hardware.
11. Do not implement config generation in v0.1.
    - No `init` command.
    - No `--example-config` option.
    - Config examples live in documentation only.

---

## Known future extensions, not v0.1

These are intentionally out of scope now:

- Subcommands, if a future UI genuinely needs them.
- `--config`; positional `CONFIG` is the only v0.1 config selector.
- Hot reload config.
- `audiorouter status`.
- `--example-config` or any other config-template printing mode.
- JSON output for device listing.
- Audio Process Tap input sources.
- Recording routes to WAV/FLAC.
- Network output.
- Soft limiter/compressor.
- Named presets/scenes.
- HAL virtual device backend.
- LaunchAgent daemon mode.
- Device matching by UID.
- Fuzzy device name matching.
- Sample-rate conversion.

Do not implement these while completing v0.1 unless explicitly requested.
