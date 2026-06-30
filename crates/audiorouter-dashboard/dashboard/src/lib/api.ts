/**
 * API client for communicating with the audiorouter backend.
 *
 * In development, Vite proxies `/api/*` to `audiorouter-dashboard-api`
 * launched by `scripts/dev.ts`. In production, audiorouter serves the same
 * endpoints alongside the built dashboard assets.
 */

import type { AudiorouterConfig } from "../types";

export interface ValidationError {
  /** Dot-path like "devices[1].name" or "routes[0].from_channels". */
  path: string;
  message: string;
}

export interface ValidationWarning {
  path: string;
  message: string;
}

export interface ConfigLoadResponse {
  config: AudiorouterConfig;
  /** Raw TOML text for the editor view. */
  raw: string;
  path: string;
}

export interface ConfigSaveRequest {
  config: AudiorouterConfig;
}

export interface ConfigSaveResponse {
  ok: boolean;
  raw: string;
  /** Server-side validation errors (empty = valid). */
  errors: ValidationError[];
}

export interface ConfigPreviewResponse {
  raw: string;
}

export interface ConfigStatusResponse {
  errors: ValidationError[];
  warnings: ValidationWarning[];
  unavailableInputs: string[];
  unavailableOutputs: string[];
  disabledRouteIndices: number[];
  missingDeviceAliases: string[];
}

/** Audio device info returned by audiorouter-dashboard-api. */
export interface AudioDevice {
  name: string;
  maxInputChannels: number;
  maxOutputChannels: number;
  isDefaultInput: boolean;
  isDefaultOutput: boolean;
}

export interface DevicesResponse {
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  all: AudioDevice[];
}

export interface ValidateResponse {
  errors: ValidationError[];
  warnings: ValidationWarning[];
}

const API_BASE = "/api";

async function fetchJSON<T>(url: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Content-Type", "application/json");
  const res = await fetch(`${API_BASE}${url}`, {
    ...init,
    headers,
  });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status}: ${text}`);
  }
  return (await res.json()) as T;
}

export const api = {
  loadConfig: () => fetchJSON<ConfigLoadResponse>("/config"),

  saveConfig: (config: AudiorouterConfig) =>
    fetchJSON<ConfigSaveResponse>("/config", {
      method: "PUT",
      body: JSON.stringify({ config } satisfies ConfigSaveRequest),
    }),

  previewConfig: (config: AudiorouterConfig) =>
    fetchJSON<ConfigPreviewResponse>("/config/preview", {
      method: "POST",
      body: JSON.stringify({ config } satisfies ConfigSaveRequest),
    }),

  statusConfig: (config: AudiorouterConfig) =>
    fetchJSON<ConfigStatusResponse>("/config/status", {
      method: "POST",
      body: JSON.stringify({ config } satisfies ConfigSaveRequest),
    }),

  validate: (config: AudiorouterConfig) =>
    fetchJSON<ValidateResponse>("/validate", {
      method: "POST",
      body: JSON.stringify({ config }),
    }),

  listDevices: () => fetchJSON<DevicesResponse>("/devices"),
};
