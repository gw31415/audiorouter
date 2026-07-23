# Web Dashboard

Launch the built-in dashboard:

```sh
audiorouter dashboard
```

By default it binds to `127.0.0.1:7822` and opens the default browser.

![dashboard demo](assets/dashboard.gif)

## Options

```sh
# Do not open a browser
audiorouter dashboard --no-open

# Use a different port
audiorouter dashboard --port 9000

# Expose on the local network by binding 0.0.0.0
audiorouter dashboard --host

# Use a specific config file
audiorouter dashboard --config ./config.toml
```

The dashboard serves the embedded React frontend and an HTTP/SSE API under
`/api/*`. For the full endpoint reference see [Dashboard API](../reference/dashboard-api.md).

## API-only binary

There is also an API-only development binary:

```sh
cargo run -p audiorouter-dashboard --bin audiorouter-dashboard-api -- \
  --addr 127.0.0.1:7822 --config ./config.toml
```

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `AUDIOROUTER_DASHBOARD_ADDR` | `127.0.0.1:7822` | Bind address for dashboard/API server binaries |
| `AUDIOROUTER_CONFIG` | platform config path | Config path used by dashboard/API server binaries |
| `SKIP_DASHBOARD_BUILD` | unset | Set to `1` to skip the frontend build in `build.rs` |
