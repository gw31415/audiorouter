# Dashboard API

The API is primarily for the bundled frontend, but is useful for development and automation:

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/config` | `GET` | Load the current config and raw TOML |
| `/api/config` | `PUT` | Validate and save a config |
| `/api/config/preview` | `POST` | Convert a config JSON payload to TOML without saving |
| `/api/config/status` | `POST` | Return validation state plus dashboard status helpers |
| `/api/validate` | `POST` | Validate config JSON |
| `/api/devices` | `GET` | List available input/output devices |
| `/api/runtime` | `GET` | Return the latest runtime snapshot |
| `/api/events` | `GET` | Server-sent events for config, device, runtime, and log changes |
