# Quick Start

Find your device names:

```sh
audiorouter list-devices
```

Print the default config location:

```sh
audiorouter config-path
```

Create a TOML config at that path, for example:

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

Validate the config, then start routing:

```sh
audiorouter check
audiorouter
```

`audiorouter` with no subcommand is equivalent to `audiorouter run`.
