/**
 * Type definitions mirroring audiorouter's Rust config structs.
 *
 * Mirrors src/config.rs and src/validate.rs.
 */

/** `[engine]` — sample rate and buffer size. */
export interface EngineConfig {
  sample_rate: number;
  buffer_size: number;
}

/** `[[devices]]` — a named device alias. */
export interface DeviceConfig {
  /**
   * Config-local alias used by routes. **May be empty** — when empty,
   * the `device` field is used as the alias at runtime.
   * Mirrors Rust `name: Option<String>` with `unwrap_or_else(|| device.clone())`.
   */
  name: string;
  /** Human-readable CoreAudio device name to match. */
  device: string;
  /** Applies only when this device is used as an output. */
  limiter: boolean;
}

/**
 * Resolve the effective alias for a device.
 * When `name` is empty, falls back to `device` (mirrors config.rs).
 */
export function effectiveName(dev: { name: string; device: string }): string {
  return dev.name || dev.device;
}

/** `[[routes]]` — a channel mapping from one device to another. */
export interface RouteConfig {
  from: string;
  to: string;
  /** Source physical input channels, 1-based. */
  from_channels: number[];
  /** Destination physical output channels, 1-based. */
  to_channels: number[];
  /** Gain applied to this route before mixing (dB). */
  gain_db: number;
  /** If true, route contributes silence. */
  mute: boolean;
}

/** Top-level config structure. */
export interface AudiorouterConfig {
  engine: EngineConfig;
  devices: DeviceConfig[];
  routes: RouteConfig[];
}

/**
 * Device with inferred input/output roles and required channel counts.
 * Mirrors `validate::ResolvedDeviceRole` in Rust.
 */
export interface ResolvedDeviceRole {
  name: string;
  device: string;
  limiter: boolean;
  needs_input: boolean;
  needs_output: boolean;
  required_input_channels: number;
  required_output_channels: number;
}

/** A validated route. Mirrors `validate::ValidatedRoute`. */
export interface ValidatedRoute extends RouteConfig {}

/** The output of successful validation. Mirrors `validate::ValidatedConfig`. */
export interface ValidatedConfig {
  config: AudiorouterConfig;
  devices: ResolvedDeviceRole[];
  routes: ValidatedRoute[];
  warnings: string[];
}

export const DEFAULT_ENGINE: EngineConfig = {
  sample_rate: 48000,
  buffer_size: 256,
};

export const DEFAULT_DEVICE: Omit<DeviceConfig, "device"> = {
  name: "",
  limiter: false,
};

export const DEFAULT_ROUTE: Omit<RouteConfig, "from" | "to" | "from_channels" | "to_channels"> = {
  gain_db: 0,
  mute: false,
};

export function createEmptyConfig(): AudiorouterConfig {
  return {
    engine: { ...DEFAULT_ENGINE },
    devices: [],
    routes: [],
  };
}
