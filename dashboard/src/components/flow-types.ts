import type { Edge, Node } from "@xyflow/react";

/** Device role inferred from routes (mirrors tui.rs semantics). */
export type DeviceRole = "input" | "output" | "both";

/** Channel info shown on the node border (mirrors tui.rs draw_device_node). */
export interface ChannelInfo {
  /** Routed input channels count (▲). */
  chIn: number;
  /** Routed output channels count (▼). */
  chOut: number;
  /** Total physical input channels (from device, if known). */
  totalIn: number;
  /** Total physical output channels (from device, if known). */
  totalOut: number;
}

export interface DeviceNodeData extends Record<string, unknown> {
  name: string;
  device: string;
  limiter: boolean;
  /** Inferred role from routes. */
  role: DeviceRole;
  /** Channel info computed from routes + device. */
  channels: ChannelInfo;
  /** True when the device is not found in the system (mirrors tui.rs `unavailable`). */
  missingInput: boolean;
  missingOutput: boolean;
}

export interface RouteEdgeData extends Record<string, unknown> {
  from: string;
  to: string;
  from_channels: number[];
  to_channels: number[];
  gain_db: number;
  mute: boolean;
  /** Whether this route is disabled (e.g. device missing). */
  disabled: boolean;
  /** Index among routes with the same from→to pair, used to avoid path overlap. */
  parallelIndex?: number;
  /** Count of routes with the same from→to pair. */
  parallelCount?: number;
}

export interface FlowState {
  nodes: Node[];
  edges: Edge[];
}
