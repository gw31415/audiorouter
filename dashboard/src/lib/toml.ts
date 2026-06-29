/**
 * TOML ↔ JSON conversion using smol-toml.
 *
 * Handles the subtle difference between the on-disk TOML representation
 * (where `device.name` is optional and defaults to `device.device`) and the
 * JSON representation (where `name` is always present).
 */
import { parse, stringify } from "smol-toml";
import type { AudiorouterConfig, DeviceConfig } from "../types";

/**
 * Parse a TOML string into an AudiorouterConfig.
 *
 * `device.name` is optional in TOML (mirrors Rust's `Option<String>`).
 * When omitted, the resulting `DeviceConfig.name` is **empty string**,
 * and the UI uses `effectiveName()` (= device) at display time.
 * TOML output (`stringifyConfig`) omits `name` when empty.
 */
export function parseConfig(toml: string): AudiorouterConfig {
  const raw = parse(toml) as unknown as RawTomlConfig;
  return normalizeConfig(raw);
}

interface RawTomlConfig {
  engine?: {
    sample_rate?: number;
    buffer_size?: number;
  };
  devices?: Array<{
    name?: string;
    device: string;
    limiter?: boolean;
  }>;
  routes?: Array<{
    from: string;
    to: string;
    from_channels: number[];
    to_channels: number[];
    gain_db?: number;
    mute?: boolean;
  }>;
}

function normalizeConfig(raw: RawTomlConfig): AudiorouterConfig {
  return {
    engine: {
      sample_rate: raw.engine?.sample_rate ?? 48000,
      buffer_size: raw.engine?.buffer_size ?? 256,
    },
    devices: (raw.devices ?? []).map((d) => ({
      name: d.name ?? "", // empty = use device as alias at runtime
      device: d.device,
      limiter: d.limiter ?? false,
    })),
    routes: (raw.routes ?? []).map((r) => ({
      from: r.from,
      to: r.to,
      from_channels: r.from_channels,
      to_channels: r.to_channels,
      gain_db: r.gain_db ?? 0,
      mute: r.mute ?? false,
    })),
  };
}

/**
 * Serialize an AudiorouterConfig to a TOML string.
 *
 * Omits `device.name` when empty or equal to `device.device`,
 * matching Rust's `Option<String>` with runtime fallback to `device`.
 * Omits `gain_db` when 0, `mute` when false, `limiter` when false,
 * and `[engine]` keys that match the defaults (48000 / 256).
 * Omits an entire `[[devices]]` entry when it is fully inferable from
 * routes: no `name` alias, `limiter = false`, and the `device` string
 * is referenced in some route's `from` or `to`.
 */
export function stringifyConfig(config: AudiorouterConfig): string {
  const tomlObj: Record<string, unknown> = {};

  // Engine — omit keys that match the defaults (48000 / 256).
  // If both are default the entire section is skipped.
  const engine: Record<string, number> = {};
  if (config.engine.sample_rate !== 48000) engine.sample_rate = config.engine.sample_rate;
  if (config.engine.buffer_size !== 256) engine.buffer_size = config.engine.buffer_size;
  if (Object.keys(engine).length > 0) tomlObj.engine = engine;

  // Collect all device names referenced by routes — a device entry
  // whose alias matches one of these and has no non-default fields is
  // implicitly created by Rust's `add_implicit_route_devices`.
  const routeAliases = new Set<string>();
  for (const r of config.routes) {
    routeAliases.add(r.from);
    routeAliases.add(r.to);
  }

  // Devices — skip entries that Rust can infer from routes.
  const devices = config.devices
    .filter((d) => !isDeviceImplicit(d, routeAliases))
    .map((d) => deviceToToml(d));
  if (devices.length > 0) tomlObj.devices = devices;

  // Routes
  if (config.routes.length > 0) {
    tomlObj.routes = config.routes.map((r) => {
      const entry: Record<string, unknown> = {
        from: r.from,
        to: r.to,
        from_channels: r.from_channels,
        to_channels: r.to_channels,
      };
      if (r.gain_db !== 0) entry.gain_db = r.gain_db;
      if (r.mute) entry.mute = true;
      return entry;
    });
  }

  return stringify(tomlObj);
}

/**
 * A `[[devices]]` entry is implicit (can be omitted) when all of:
 *   - `name` is empty or equals `device` (no custom alias)
 *   - `limiter` is false (default)
 *   - The device string appears in a route's `from` or `to`
 *
 * Rust's `add_implicit_route_devices` (validate.rs:179) recreates such
 * entries automatically: `name = device = <route reference>, limiter = false`.
 */
function isDeviceImplicit(d: DeviceConfig, routeAliases: Set<string>): boolean {
  const alias = d.name === "" || d.name === d.device ? d.device : d.name;
  return (d.name === "" || d.name === d.device) && !d.limiter && routeAliases.has(alias);
}

function deviceToToml(d: DeviceConfig): Record<string, unknown> {
  const entry: Record<string, unknown> = { device: d.device };
  // Only write `name` when non-empty AND differs from `device`
  if (d.name !== "" && d.name !== d.device) entry.name = d.name;
  if (d.limiter) entry.limiter = true;
  return entry;
}
