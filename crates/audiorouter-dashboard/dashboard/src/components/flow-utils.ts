import type { Edge, Node } from "@xyflow/react";
import { computeLayout } from "../lib/graph";
import type { AudiorouterConfig } from "../types";
import { effectiveName } from "../types";
import type { DeviceRole, FlowState } from "./flow-types";
import type { ChannelInfo, DeviceNodeData, RouteEdgeData } from "./flow-types";

/** Infer device role from routes (mirrors validate.rs role inference). */
function inferRole(name: string, routes: { from: string; to: string }[]): DeviceRole {
  let needsInput = false;
  let needsOutput = false;
  for (const r of routes) {
    if (r.from === name) needsInput = true;
    if (r.to === name) needsOutput = true;
  }
  if (needsInput && needsOutput) return "both";
  if (needsInput) return "input";
  if (needsOutput) return "output";
  return "input"; // no routes — placeholder
}

/**
 * Compute channel info for a device (mirrors tui.rs draw_device_node).
 *
 * Counts unique channels actually routed to/from this device across all
 * active routes. If total channels are known (from device info), shows
 * "used/total"; otherwise shows just "used".
 */
function computeChannelInfo(
  name: string,
  config: AudiorouterConfig,
  totalIn: number = 0,
  totalOut: number = 0,
): ChannelInfo {
  const activeInputChannels = new Set<number>();
  const activeOutputChannels = new Set<number>();

  for (const r of config.routes) {
    if (r.from === name) {
      for (const ch of r.from_channels) activeInputChannels.add(ch);
    }
    if (r.to === name) {
      for (const ch of r.to_channels) activeOutputChannels.add(ch);
    }
  }

  return {
    chIn: activeInputChannels.size,
    chOut: activeOutputChannels.size,
    totalIn,
    totalOut,
  };
}

/** Layout constants (mirror tui.rs draw_routing_graph). */
const NODE_W = 220;
const NODE_H = 100;
// Wider reset-layout spacing than the TUI grid: React Flow edges carry
// floating labels and parallel offsets, so tight rows/columns cause overlaps.
const ROW_GAP = 70;
const COL_GAP = 180;

/**
 * Convert validated config → React Flow nodes + edges.
 *
 * Uses the topological layered layout from graph.ts (Sugiyama framework),
 * mirroring how audiorouter's TUI places devices.
 */
export function configToFlow(config: AudiorouterConfig): FlowState {
  // Use effective names for layout (name="" → device).
  // Mirrors validate.rs::add_implicit_route_devices: route endpoints that are
  // not listed in [[devices]] are still real implicit devices. They must be
  // included so missing hardware like "VT-4" (present only in [[routes]]) can
  // be hidden/shown by the Missing toggle.
  const deviceNames = Array.from(
    new Set([
      ...config.devices.map((d) => effectiveName(d)),
      ...config.routes.flatMap((r) => [r.from, r.to]),
    ]),
  );
  const layout = computeLayout(
    deviceNames,
    config.routes.map((r) => ({ from: r.from, to: r.to })),
    new Set<string>(),
  );

  // `computeLayout()` intentionally lays out only route-participating devices
  // (matching the TUI graph). The dashboard, however, must preserve configured
  // but disconnected devices so the Disconnected toggle can reveal them.
  // Append such devices to layer 0 after the active nodes.
  const activeLayout =
    layout.length > 0
      ? layout
      : deviceNames
          .filter((name) => config.routes.some((r) => r.from === name || r.to === name))
          .map((name, i) => ({
            alias: name,
            layer: 0,
            row: i,
          }));

  const laidOut = new Set(activeLayout.map((n) => n.alias));
  const layer0Rows = activeLayout.filter((n) => n.layer === 0).map((n) => n.row);
  const disconnectedStartRow = layer0Rows.length > 0 ? Math.max(...layer0Rows) + 1 : 0;
  const disconnectedLayout = deviceNames
    .filter((name) => !laidOut.has(name))
    .map((name, i) => ({
      alias: name,
      layer: 0,
      row: disconnectedStartRow + i,
    }));

  const effectiveLayout = [...activeLayout, ...disconnectedLayout];

  // Build node positions
  const nodes: Node[] = [];
  const nodeIndex = new Map<string, number>();

  for (const placed of effectiveLayout) {
    // Find device by effective name
    const dev = config.devices.find((d) => effectiveName(d) === placed.alias);
    const role = inferRole(
      placed.alias,
      config.routes.map((r) => ({ from: r.from, to: r.to })),
    );
    const channels = computeChannelInfo(placed.alias, config);

    const data: DeviceNodeData = {
      name: dev?.name ?? "", // raw alias (may be empty)
      device: dev?.device ?? placed.alias,
      limiter: dev?.limiter ?? false,
      role,
      channels,
      missingInput: false,
      missingOutput: false,
    };

    nodeIndex.set(placed.alias, nodes.length);
    nodes.push({
      id: `device-${placed.alias}`,
      type: "device",
      position: {
        x: placed.layer * (NODE_W + COL_GAP),
        y: placed.row * (NODE_H + ROW_GAP),
      },
      data,
    });
  }

  // Build edges. Track parallel routes so RouteEdge can offset paths and labels
  // instead of drawing multiple routes directly on top of one another.
  const parallelTotals = new Map<string, number>();
  const parallelSeen = new Map<string, number>();
  for (const r of config.routes) {
    const key = `${r.from}\u0000${r.to}`;
    parallelTotals.set(key, (parallelTotals.get(key) ?? 0) + 1);
  }

  const edges: Edge[] = config.routes.map((r, i) => {
    const parallelKey = `${r.from}\u0000${r.to}`;
    const parallelIndex = parallelSeen.get(parallelKey) ?? 0;
    parallelSeen.set(parallelKey, parallelIndex + 1);
    const parallelCount = parallelTotals.get(parallelKey) ?? 1;

    const data: RouteEdgeData = {
      from: r.from,
      to: r.to,
      from_channels: r.from_channels,
      to_channels: r.to_channels,
      gain_db: r.gain_db,
      mute: r.mute,
      disabled: false,
      parallelIndex,
      parallelCount,
    };

    const label = formatEdgeLabel(r, false);

    return {
      id: `route-${i}`,
      source: `device-${r.from}`,
      target: `device-${r.to}`,
      sourceHandle: "out",
      targetHandle: "in",
      type: "route",
      animated: !r.mute,
      data,
      label,
      labelStyle: edgeLabelStyle(r.mute, false),
      labelBgStyle: { fill: "var(--color-card)", fillOpacity: 1 },
      style: edgeStrokeStyle(r.mute, false),
    } satisfies Edge;
  });

  return { nodes, edges };
}

/** Format the edge label, mirroring tui.rs draw_edge. */
function formatEdgeLabel(route: { gain_db: number }, disabled: boolean): string {
  if (disabled) return "OFF";
  if (route.gain_db === 0) return "──────";
  return `${route.gain_db > 0 ? "+" : ""}${route.gain_db.toFixed(1)}dB`;
}

function edgeLabelStyle(mute: boolean, disabled: boolean): object {
  if (disabled || mute) {
    return {
      fontSize: 11,
      fill: "var(--color-ar-disabled)",
      fontWeight: 500,
    };
  }
  return {
    fontSize: 11,
    fill: "var(--color-ar-gain)",
    fontWeight: 600,
  };
}

function edgeStrokeStyle(mute: boolean, disabled: boolean): object {
  if (disabled || mute) {
    return {
      stroke: "var(--color-ar-disabled)",
      strokeWidth: 1.5,
      strokeDasharray: "4 3",
    };
  }
  return {
    stroke: "var(--color-ar-route)",
    strokeWidth: 2,
  };
}

/**
 * Convert React Flow state back to audiorouter config.
 *
 * Node IDs use effective names. When converting back, `name` stays as-is
 * from the raw DeviceNodeData (may be empty string).
 */
export function flowToConfig(
  flow: FlowState,
  engine: AudiorouterConfig["engine"],
): AudiorouterConfig {
  const devices: AudiorouterConfig["devices"] = [];
  const routes: AudiorouterConfig["routes"] = [];

  // Devices — deduplicate by effective name, preserve config order
  const seenEffectiveNames = new Set<string>();
  for (const node of flow.nodes) {
    const data = node.data as DeviceNodeData;
    const effName = data.name || data.device;
    if (seenEffectiveNames.has(effName)) continue;
    seenEffectiveNames.add(effName);
    devices.push({
      name: data.name, // raw — may be empty
      device: data.device,
      limiter: data.limiter,
    });
  }

  // Routes from edges — use effective names for from/to
  for (const edge of flow.edges) {
    const data = edge.data as RouteEdgeData | undefined;
    if (!data) continue;
    routes.push({
      from: data.from,
      to: data.to,
      from_channels: data.from_channels,
      to_channels: data.to_channels,
      gain_db: data.gain_db,
      mute: data.mute,
    });
  }

  return { engine, devices, routes };
}

/**
 * Create a new route edge when the user drags a connection between nodes.
 * Route aliases use effective names (name || device).
 */
export function createRouteEdge(sourceNode: Node, targetNode: Node, existingCount: number): Edge {
  const sourceData = sourceNode.data as DeviceNodeData;
  const targetData = targetNode.data as DeviceNodeData;

  const fromAlias = sourceData.name || sourceData.device;
  const toAlias = targetData.name || targetData.device;

  const data: RouteEdgeData = {
    from: fromAlias,
    to: toAlias,
    from_channels: [1],
    to_channels: [1],
    gain_db: 0,
    mute: false,
    disabled: false,
    parallelIndex: 0,
    parallelCount: 1,
  };

  return {
    id: `route-${Date.now()}-${existingCount}`,
    source: sourceNode.id,
    target: targetNode.id,
    sourceHandle: "out",
    targetHandle: "in",
    type: "route",
    animated: true,
    data,
    label: "──────",
    labelStyle: edgeLabelStyle(false, false),
    labelBgStyle: { fill: "var(--color-card)", fillOpacity: 1 },
    style: edgeStrokeStyle(false, false),
  };
}

/**
 * Re-apply the Sugiyama layered layout to the current flow state.
 * Used by the "format" button in the canvas toolbar.
 */
export function applyAutoLayout(nodes: Node[], edges: Edge[]): Node[] {
  const deviceAliases = nodes.map((n) => {
    const d = n.data as DeviceNodeData;
    return d.name || d.device;
  });
  const routes = edges.map((e) => {
    const d = e.data as RouteEdgeData;
    return { from: d.from, to: d.to };
  });

  const layout = computeLayout(deviceAliases, routes, new Set<string>());
  const posMap = new Map<string, { x: number; y: number }>();
  for (const p of layout) {
    posMap.set(p.alias, {
      x: p.layer * (NODE_W + COL_GAP),
      y: p.row * (NODE_H + ROW_GAP),
    });
  }

  // Stack disconnected nodes (not participating in routes) below the main layout
  const laidOut = new Set(layout.map((p) => p.alias));
  const maxY = layout.length > 0 ? Math.max(...layout.map((p) => p.row)) * (NODE_H + ROW_GAP) : 0;
  deviceAliases
    .filter((a) => !laidOut.has(a))
    .forEach((a, i) => {
      posMap.set(a, { x: 0, y: maxY + (NODE_H + ROW_GAP) * (i + 1) });
    });

  return nodes.map((n) => {
    const d = n.data as DeviceNodeData;
    const alias = d.name || d.device;
    const pos = posMap.get(alias);
    return pos ? { ...n, position: pos } : n;
  });
}

/** Re-compute channel info for all nodes after a route change. */
export function recomputeNodeData(nodes: Node[], config: AudiorouterConfig): Node[] {
  return nodes.map((node) => {
    const data = node.data as DeviceNodeData;
    const alias = data.name || data.device;
    const role = inferRole(
      alias,
      config.routes.map((r) => ({ from: r.from, to: r.to })),
    );
    const channels = computeChannelInfo(alias, config);
    return {
      ...node,
      data: { ...data, role, channels },
    };
  });
}
