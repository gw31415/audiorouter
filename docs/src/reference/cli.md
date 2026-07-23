# CLI Reference

## `audiorouter --help`

```text
Cross-platform command-line audio router with a terminal UI

Usage: audiorouter [OPTIONS] [COMMAND]

Commands:
  run           Start audio routing (default when no subcommand is given)
  check         Validate configuration and device availability, then exit
  list-devices  List available audio input/output devices, then exit
  config-path   Print the resolved configuration path, then exit
  dashboard     Launch the web dashboard (HTTP/SSE UI) in the default browser
  completions   Generate a shell completion script
  help          Print this message or the help of the given subcommand(s)

Options:
  -c, --config <CONFIG>
          TOML configuration file to read.
          
          If omitted, audiorouter reads the default XDG config path.

  -q, --quiet
          Suppress non-error output

  -v, --verbose...
          Print extra diagnostics. Repeat for more detail.
          
          -v:   debug-level tracing (open/close device, route resolution)
          -vv:  trace-level (per-callback timing, underrun counts)

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## `audiorouter dashboard --help`

```text
Launch the web dashboard (HTTP/SSE UI) in the default browser.

By default the dashboard binds to localhost (127.0.0.1) on port 7822.

Usage: audiorouter dashboard [OPTIONS]

Options:
  -c, --config <CONFIG>
          TOML configuration file to read.
          
          If omitted, audiorouter reads the default XDG config path.

      --host
          Expose the dashboard on the local network (bind 0.0.0.0).
          
          Off by default, which keeps the dashboard reachable only from this
          machine. Pass `--host` to share it with other devices on the LAN.

  -p, --port <PORT>
          Port to bind the dashboard server on
          
          [default: 7822]

  -q, --quiet
          Suppress non-error output

      --no-open
          Do not open the dashboard in the default browser

  -v, --verbose...
          Print extra diagnostics. Repeat for more detail.
          
          -v:   debug-level tracing (open/close device, route resolution)
          -vv:  trace-level (per-callback timing, underrun counts)

  -h, --help
          Print help (see a summary with '-h')
```

## `audiorouter completions --help`

```text
Generate a shell completion script.

Writes to stdout by default; use --output to write to a file instead.
When no shell is given the current shell is detected from $SHELL.

Usage: audiorouter completions [OPTIONS] [SHELL]

Arguments:
  [SHELL]
          Shell to generate completions for [default: current $SHELL]
          
          [possible values: bash, elvish, fish, powershell, zsh]

Options:
  -c, --config <CONFIG>
          TOML configuration file to read.
          
          If omitted, audiorouter reads the default XDG config path.

  -o, --output <OUTPUT>
          Output file [default: stdout]

  -q, --quiet
          Suppress non-error output

  -v, --verbose...
          Print extra diagnostics. Repeat for more detail.
          
          -v:   debug-level tracing (open/close device, route resolution)
          -vv:  trace-level (per-callback timing, underrun counts)

  -h, --help
          Print help (see a summary with '-h')
```

## Examples

Use an explicit config file:

```sh
audiorouter --config ./config.toml check
audiorouter run -c ./config.toml
```

Increase diagnostics:

```sh
audiorouter -v check
# -v  = debug-level tracing
# -vv = trace-level diagnostics
```

Generate shell completions:

```sh
# Write to stdout and source immediately (fish example)
audiorouter completions fish | source

# Write to a file
audiorouter completions bash --output ~/.bash_completion.d/audiorouter
```
