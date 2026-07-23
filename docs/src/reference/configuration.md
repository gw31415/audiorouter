# Configuration

## Configuration Path

Resolution order when `--config` is not provided:

1. `$XDG_CONFIG_HOME/audiorouter/config.toml` — if `XDG_CONFIG_HOME` is set and non-empty
2. `~/.config/audiorouter/config.toml` — if the file already exists there
3. Platform-native config directory:
   - **Linux/BSD** `~/.config/audiorouter/config.toml`
   - **macOS** `~/Library/Application Support/audiorouter/config.toml`
   - **Windows** `%APPDATA%\audiorouter\config.toml`

Print the resolved path for your system:

```sh
audiorouter config-path
```

Relative paths passed with `--config` are resolved against the current working
directory.

## Config Reference

### `[engine]`

| Key | Default | Description |
|-----|---------|-------------|
| `sample_rate` | `48000` | Sample rate in Hz |
| `buffer_size` | `256` | Buffer size in frames |

The entire `[engine]` table is optional. Missing fields use their defaults.

### `[[devices]]`

| Key | Required | Description |
|-----|----------|-------------|
| `device` | yes | Exact device name as reported by `list-devices` |
| `name` | no | Config-local alias used in routes; defaults to `device` |
| `limiter` | no | Enable peak limiter when this device is used as an output; default `false` |

### `[[routes]]`

| Key | Required | Description |
|-----|----------|-------------|
| `from` | yes | Source device alias |
| `to` | yes | Destination device alias |
| `from_channels` | yes | 1-based source channel list |
| `to_channels` | yes | 1-based destination channel list |
| `gain_db` | no | Gain in dB; default `0.0` |
| `mute` | no | Mute this route; default `false` |

`from_channels` and `to_channels` must have the same length. Repeat a channel
index to duplicate it, for example:

```toml
from_channels = [1, 1]
to_channels   = [1, 2]
```

That maps mono input channel 1 to stereo output channels 1 and 2.

## Larger Example

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
mute = false
```
