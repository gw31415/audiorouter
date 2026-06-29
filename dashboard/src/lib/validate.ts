/**
 * Pure config validation and device role inference.
 *
 * Direct port of `src/validate.rs`. All validation here is about internal
 * config consistency — device aliases, channel numbers, route references,
 * and role/channel-count inference.
 */
import type {
  AudiorouterConfig,
  DeviceConfig,
  ResolvedDeviceRole,
  RouteConfig,
  ValidatedConfig,
  ValidatedRoute,
} from "../types";
import { effectiveName } from "../types";

export interface ValidationError {
  /** Dot-path like "devices[1].name" or "routes[0].from_channels". */
  path: string;
  message: string;
}

export interface ValidationWarning {
  path: string;
  message: string;
}

/**
 * Add implicit devices for route endpoints that are not explicitly defined.
 * Mirrors `validate::add_implicit_route_devices`.
 *
 * Route aliases reference effective names. A route from/to a name that doesn't
 * match any device's effective name gets an implicit device added.
 */
function addImplicitRouteDevices(devices: DeviceConfig[], routes: RouteConfig[]): DeviceConfig[] {
  const known = new Set(devices.map((d) => effectiveName(d)));
  const result = [...devices];

  for (const route of routes) {
    for (const alias of [route.from, route.to]) {
      if (!known.has(alias)) {
        known.add(alias);
        result.push({
          name: "", // implicit device — no alias override
          device: alias,
          limiter: false,
        });
      }
    }
  }
  return result;
}

/**
 * Run pure config validation.
 *
 * Returns `{ errors, validated }`:
 * - `errors` is empty on success
 * - `validated` is the full ValidatedConfig on success, null otherwise
 */
export function validateConfigFull(config: AudiorouterConfig): {
  errors: ValidationError[];
  validated: ValidatedConfig | null;
} {
  const errors: ValidationError[] = [];
  const warnings: ValidationWarning[] = [];

  // ── Engine ──
  if (config.engine.sample_rate === 0) {
    errors.push({
      path: "engine.sample_rate",
      message: "engine.sample_rate must be positive",
    });
  }
  if (config.engine.buffer_size === 0) {
    errors.push({
      path: "engine.buffer_size",
      message: "engine.buffer_size must be positive",
    });
  }

  // Add implicit devices from routes
  const allDevices = addImplicitRouteDevices(config.devices, config.routes);

  // ── Device validation ──
  // Use effective names for duplicate detection (name="" → device)
  const nameMap = new Map<string, DeviceConfig>();
  for (const dev of allDevices) {
    const alias = effectiveName(dev);
    // Only validate the 'device' (hardware name) field — 'name' is optional
    if (dev.device.trim() === "") {
      errors.push({
        path: `devices`,
        message: `device "${alias}" has an empty 'device' field (audio device name)`,
      });
    }
    if (nameMap.has(alias)) {
      errors.push({
        path: `devices`,
        message: `duplicate device alias "${alias}"; names must be unique`,
      });
    } else {
      nameMap.set(alias, dev);
    }
  }

  // ── Route validation ──
  if (config.routes.length === 0) {
    errors.push({
      path: "routes",
      message: "config must define at least one route",
    });
  }

  for (let i = 0; i < config.routes.length; i++) {
    validateRoute(i, config.routes[i], nameMap, errors);
  }

  if (errors.length > 0) {
    return { errors, validated: null };
  }

  // ── Role inference ──
  const roles = new Map<string, ResolvedDeviceRole>();
  for (const d of allDevices) {
    const alias = effectiveName(d);
    roles.set(alias, {
      name: alias,
      device: d.device,
      limiter: d.limiter,
      needs_input: false,
      needs_output: false,
      required_input_channels: 0,
      required_output_channels: 0,
    });
  }

  for (const route of config.routes) {
    const fromRole = roles.get(route.from);
    if (fromRole) {
      fromRole.needs_input = true;
      for (const ch of route.from_channels) {
        if (ch > fromRole.required_input_channels) fromRole.required_input_channels = ch;
      }
    }
    const toRole = roles.get(route.to);
    if (toRole) {
      toRole.needs_output = true;
      for (const ch of route.to_channels) {
        if (ch > toRole.required_output_channels) toRole.required_output_channels = ch;
      }
    }
  }

  // ── Warnings ──
  if (!sampleRateInRecommendedRange(config.engine.sample_rate)) {
    warnings.push({
      path: "engine.sample_rate",
      message: `engine.sample_rate ${config.engine.sample_rate} is outside the recommended range (44100 or 48000)`,
    });
  }
  if (!bufferSizeInRecommendedRange(config.engine.buffer_size)) {
    warnings.push({
      path: "engine.buffer_size",
      message: `engine.buffer_size ${config.engine.buffer_size} is outside the recommended range (64..=2048)`,
    });
  }

  const validated: ValidatedConfig = {
    config,
    devices: allDevices.map((d) => roles.get(effectiveName(d))!),
    routes: config.routes.map((r): ValidatedRoute => ({ ...r })),
    warnings: warnings.map((w) => w.message),
  };

  return { errors: [], validated };
}

function validateRoute(
  i: number,
  route: RouteConfig,
  nameMap: Map<string, DeviceConfig>,
  errors: ValidationError[],
): void {
  const prefix = `routes[${i}]`;
  const known = [...nameMap.keys()];

  if (!nameMap.has(route.from)) {
    errors.push({
      path: `${prefix}.from`,
      message: `route[${i}].from references unknown device alias "${route.from}"; known devices: ${known.join(", ")}`,
    });
  }
  if (!nameMap.has(route.to)) {
    errors.push({
      path: `${prefix}.to`,
      message: `route[${i}].to references unknown device alias "${route.to}"; known devices: ${known.join(", ")}`,
    });
  }

  if (route.from === route.to) {
    errors.push({
      path: prefix,
      message: `route[${i}].from and route[${i}].to are both "${route.from}"; same-device routes are rejected to prevent feedback`,
    });
  }

  if (route.from_channels.length === 0) {
    errors.push({
      path: `${prefix}.from_channels`,
      message: `route[${i}].from_channels is empty`,
    });
  }
  if (route.to_channels.length === 0) {
    errors.push({
      path: `${prefix}.to_channels`,
      message: `route[${i}].to_channels is empty`,
    });
  }

  if (route.from_channels.length !== route.to_channels.length) {
    errors.push({
      path: prefix,
      message: `route[${i}] maps from_channels length ${route.from_channels.length} to to_channels length ${route.to_channels.length}; lengths must match. Use from_channels = [1, 1] for mono-to-stereo.`,
    });
  }

  for (const ch of route.from_channels) {
    if (ch === 0) {
      errors.push({
        path: `${prefix}.from_channels`,
        message: `route[${i}].from_channels contains invalid channel 0; channels are 1-based`,
      });
    }
  }
  for (const ch of route.to_channels) {
    if (ch === 0) {
      errors.push({
        path: `${prefix}.to_channels`,
        message: `route[${i}].to_channels contains invalid channel 0; channels are 1-based`,
      });
    }
  }

  if (!Number.isFinite(route.gain_db)) {
    errors.push({
      path: `${prefix}.gain_db`,
      message: `route[${i}].gain_db is not a finite number (NaN or infinity rejected)`,
    });
  }
}

// ── Recommended-range checks (mirror validate.rs EngineConfigExt) ──

function sampleRateInRecommendedRange(rate: number): boolean {
  return rate === 44100 || rate === 48000;
}

function bufferSizeInRecommendedRange(size: number): boolean {
  return size >= 64 && size <= 2048;
}

// ── Convenience wrappers for client-side validation display ──

/** Return errors only. */
export function validateConfig(config: AudiorouterConfig): ValidationError[] {
  return validateConfigFull(config).errors;
}

/** Return warnings as path/message pairs. */
export function warnConfig(config: AudiorouterConfig): ValidationWarning[] {
  const { validated } = validateConfigFull(config);
  if (!validated) return [];

  const warnings: ValidationWarning[] = [];
  for (const w of validated.warnings) {
    // Try to match a path
    if (w.includes("sample_rate")) {
      warnings.push({ path: "engine.sample_rate", message: w });
    } else if (w.includes("buffer_size")) {
      warnings.push({ path: "engine.buffer_size", message: w });
    } else {
      warnings.push({ path: "engine", message: w });
    }
  }
  return warnings;
}
