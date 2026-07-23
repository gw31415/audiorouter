# Development

This repository is a Cargo workspace:

- `audiorouter` — CLI, terminal UI, routing runtime
- `crates/audiorouter-core` — shared config, validation, device inventory, watchers, and API DTOs
- `crates/audiorouter-dashboard` — dashboard HTTP/SSE API and embedded frontend host
- `crates/audiorouter-dashboard/dashboard` — React/Vite dashboard frontend

## Rust

```sh
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Dashboard frontend

```sh
cd crates/audiorouter-dashboard/dashboard
pnpm install
pnpm dev
```

`pnpm dev` starts both:

- `audiorouter-dashboard-api` at `AUDIOROUTER_DASHBOARD_ADDR` or `127.0.0.1:7822`
- the Vite dev server, proxying `/api/*` to that API server

Frontend checks:

```sh
cd crates/audiorouter-dashboard/dashboard
pnpm check
pnpm test
pnpm lint
pnpm format
```

Build the embedded dashboard host directly:

```sh
cargo run -p audiorouter-dashboard
```

For pure Rust iterations where a previously built dashboard `dist` can be reused:

```sh
SKIP_DASHBOARD_BUILD=1 cargo test --workspace
```
