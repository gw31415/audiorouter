# audiorouter

[![crates.io](https://img.shields.io/crates/v/audiorouter.svg)](https://crates.io/crates/audiorouter)

A cross-platform command-line audio router. Reads a TOML configuration file, opens named audio devices, remaps and mixes audio channels in real time, and writes the result into virtual or physical output devices.

## Features

- Real-time channel remapping and mixing (many-to-one, one-to-many, mono-to-stereo)
- Per-route gain (dB) and mute
- Optional per-output peak limiter
- Terminal UI with live VU meters and route graph
- Config file watching with live reload
- Shell completion generation (Bash, Fish, Zsh, …)
- XDG config path with platform-native fallback

## Installation

```sh
cargo binstall audiorouter
```

## Quick Start

Find your device names:

```sh
audiorouter list-devices
```

Create a config file at the [default path](#configuration-path):

```toml
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "mic"
device = "MacBook Pro Microphone"

[[devices]]
name = "out"
device = "BlackHole 2ch"
limiter = true

[[routes]]
from = "mic"
to = "out"
from_channels = [1, 1]   # mono → stereo
to_channels   = [1, 2]
gain_db = -6.0
```

Validate the config, then run:

```sh
audiorouter check
audiorouter
```

## Usage

```
Usage: audiorouter [OPTIONS] [COMMAND]

Commands:
  run           Start audio routing (default)
  check         Validate config and device availability, then exit
  list-devices  List available audio input/output devices
  config-path   Print the resolved configuration path
  completions   Generate a shell completion script

Options:
  -c, --config <FILE>  TOML configuration file to read
  -q, --quiet          Suppress non-error output
  -v, --verbose        Print extra diagnostics (-vv for trace level)
  -h, --help           Print help
  -V, --version        Print version
```

### Shell completions

```sh
# Write to stdout and source immediately (fish example)
audiorouter completions fish | source

# Write to a file
audiorouter completions bash --output ~/.bash_completion.d/audiorouter
```

## Configuration

### Configuration Path

Resolution order:

1. `$XDG_CONFIG_HOME/audiorouter/config.toml` — if `XDG_CONFIG_HOME` is set
2. `~/.config/audiorouter/config.toml` — if the file already exists there
3. Platform-native:
   - **Linux/BSD** `~/.config/audiorouter/config.toml`
   - **macOS** `~/Library/Application Support/audiorouter/config.toml`
   - **Windows** `%APPDATA%\audiorouter\config.toml`

Print the resolved path for your system:

```sh
audiorouter config-path
```

### Config Reference

#### `[engine]`

| Key | Default | Description |
|-----|---------|-------------|
| `sample_rate` | `48000` | Sample rate in Hz |
| `buffer_size` | `256` | Buffer size in frames |

#### `[[devices]]`

| Key | Required | Description |
|-----|----------|-------------|
| `device` | yes | Exact device name as reported by `list-devices` |
| `name` | no | Alias used in routes (defaults to `device`) |
| `limiter` | no | Enable peak limiter on output (default `false`) |

#### `[[routes]]`

| Key | Required | Description |
|-----|----------|-------------|
| `from` | yes | Source device alias |
| `to` | yes | Destination device alias |
| `from_channels` | yes | 1-based source channel list |
| `to_channels` | yes | 1-based destination channel list |
| `gain_db` | no | Gain in dB (default `0.0`) |
| `mute` | no | Mute this route (default `false`) |

`from_channels` and `to_channels` must have the same length. Repeat a channel index to duplicate it (e.g. mono-to-stereo: `from_channels = [1, 1]`, `to_channels = [1, 2]`).

## License

Apache License 2.0 — see [LICENSE](LICENSE).
