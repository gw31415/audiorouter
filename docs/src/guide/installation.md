# Installation

## Pre-built binary (recommended)

```sh
cargo binstall audiorouter
```

This downloads a pre-built binary from the [GitHub Releases][releases] page —
no compilation step required.

[releases]: https://github.com/gw31415/audiorouter/releases

## Build from source

```sh
git clone https://github.com/gw31415/audiorouter.git
cd audiorouter
cargo install --path .
```

> The dashboard frontend is embedded into the Rust binary at build time.
> Building from source requires Node.js and pnpm unless you are intentionally
> reusing an existing dashboard build with `SKIP_DASHBOARD_BUILD=1`.

## Prerequisites

| Platform | Notes |
|----------|-------|
| Linux | ALSA headers required (`libasound2-dev` on Debian/Ubuntu) |
| macOS | No extra dependencies |
| Windows | No extra dependencies |
