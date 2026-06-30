/**
 * Topological layered layout for the routing graph.
 *
 * Direct port of `src/graph.rs` — Sugiyama framework with barycenter
 * crossing minimization. Devices are placed in layers derived from
 * the actual directed graph of routes, not a fixed Input/Both/Output grid.
 */

/** A device placed in the computed layered layout. Mirrors `PlacedNode`. */
export interface PlacedNode {
  alias: string;
  /** Layer (column) index, 0-based from left. */
  layer: number;
  /** Row within the layer, 0-based from top. */
  row: number;
}

/** Return device aliases that participate in at least one route. */
export function activeDeviceNames(
  devices: string[],
  routes: { from: string; to: string }[],
): Set<string> {
  const active = new Set<string>();
  for (const r of routes) {
    active.add(r.from);
    active.add(r.to);
  }
  return active;
}

/** Device aliases configured but NOT participating in any route. */
export function disconnectedDeviceNames(
  deviceNames: string[],
  routes: { from: string; to: string }[],
): string[] {
  const active = activeDeviceNames(deviceNames, routes);
  return deviceNames.filter((name) => !active.has(name));
}

/**
 * Compute a topological layered layout for devices that participate in
 * at least one route, excluding any devices in the `exclude` set.
 *
 * Simplified Sugiyama framework:
 * 1. Layering — longest-path from source nodes
 * 2. Crossing minimization — barycenter heuristic (forward + backward pass)
 */
export function computeLayout(
  deviceNames: string[],
  routes: { from: string; to: string }[],
  exclude: Set<string>,
): PlacedNode[] {
  const active = activeDeviceNames(deviceNames, routes);
  const names = deviceNames.filter((name) => active.has(name) && !exclude.has(name));
  if (names.length === 0) return [];

  // ── Build adjacency lists (deduplicated, skip excluded) ──
  const successors = new Map<string, string[]>();
  const predecessors = new Map<string, string[]>();
  for (const r of routes) {
    if (exclude.has(r.from) || exclude.has(r.to)) continue;
    {
      const list = successors.get(r.from) ?? [];
      if (!list.includes(r.to)) list.push(r.to);
      successors.set(r.from, list);
    }
    {
      const list = predecessors.get(r.to) ?? [];
      if (!list.includes(r.from)) list.push(r.from);
      // Store predecessors under the target node. Using r.from here collapses
      // layers and causes reset-layout paths to overlap.
      predecessors.set(r.to, list);
    }
  }

  // ── Longest-path layering ──────────────────────────────
  const layer = new Map<string, number>();
  for (const name of names) layer.set(name, 0);

  const layerCap = names.length;
  const maxIters = names.length + 1;

  for (let iter = 0; iter < maxIters; iter++) {
    let changed = false;
    for (const name of names) {
      const preds = predecessors.get(name);
      if (preds && preds.length > 0) {
        const maxPred = Math.max(...preds.map((p) => layer.get(p) ?? 0));
        const newLayer = Math.min(maxPred + 1, layerCap);
        if ((layer.get(name) ?? 0) !== newLayer) {
          layer.set(name, newLayer);
          changed = true;
        }
      }
    }
    if (!changed) break;
  }

  // Remap layers to contiguous 0..=max range
  const sortedLayers = [...new Set(layer.values())].sort((a, b) => a - b);
  const remap = new Map<number, number>();
  sortedLayers.forEach((l, i) => remap.set(l, i));
  for (const name of names) {
    layer.set(name, remap.get(layer.get(name)!)!);
  }

  const maxLayer = Math.max(...layer.values(), 0);

  // ── Group nodes by layer ───────────────────────────────
  const byLayer: string[][] = Array.from({ length: maxLayer + 1 }, () => []);
  for (const name of names) {
    byLayer[layer.get(name)!].push(name);
  }

  // ── Barycenter crossing minimization ───────────────────
  // Forward pass: reorder each layer by predecessor barycenter
  for (let l = 1; l <= maxLayer; l++) {
    const prevPos = new Map<string, number>();
    byLayer[l - 1].forEach((n, i) => prevPos.set(n, i));

    byLayer[l].sort((a, b) => {
      const ba = barycenter(a, predecessors, prevPos);
      const bb = barycenter(b, predecessors, prevPos);
      return ba - bb;
    });
  }

  // Backward pass: reorder by successor barycenter
  for (let l = maxLayer - 1; l >= 0; l--) {
    const nextPos = new Map<string, number>();
    byLayer[l + 1].forEach((n, i) => nextPos.set(n, i));

    byLayer[l].sort((a, b) => {
      const ba = barycenter(a, successors, nextPos);
      const bb = barycenter(b, successors, nextPos);
      return ba - bb;
    });
  }

  // ── Build result ───────────────────────────────────────
  const result: PlacedNode[] = [];
  for (let l = 0; l <= maxLayer; l++) {
    for (let r = 0; r < byLayer[l].length; r++) {
      result.push({ alias: byLayer[l][r], layer: l, row: r });
    }
  }
  return result;
}

/**
 * Cascade-hide devices that lose all surviving routes when `initialHidden`
 * devices are removed from the graph.
 *
 * Direct port of `src/graph.rs::cascade_hidden`.
 *
 * After excluding the initial hidden set, only routes whose **both** endpoints
 * are still visible survive. Any active device that no longer appears in a
 * surviving route is also hidden.
 */
export function cascadeHidden(
  deviceNames: string[],
  routes: { from: string; to: string }[],
  initialHidden: Set<string>,
): Set<string> {
  const surviving = new Set<string>();
  for (const r of routes) {
    if (initialHidden.has(r.from) || initialHidden.has(r.to)) continue;
    surviving.add(r.from);
    surviving.add(r.to);
  }
  const active = activeDeviceNames(deviceNames, routes);
  const hidden = new Set(initialHidden);
  for (const name of active) {
    if (!surviving.has(name)) hidden.add(name);
  }
  return hidden;
}

/** Average position of `node`'s neighbours that appear in `positions`. */
function barycenter(
  node: string,
  adjacency: Map<string, string[]>,
  positions: Map<string, number>,
): number {
  const neighbors = adjacency.get(node);
  if (!neighbors || neighbors.length === 0) return Number.MAX_SAFE_INTEGER;

  const relevant = neighbors
    .map((n) => positions.get(n))
    .filter((v): v is number => v !== undefined);

  if (relevant.length === 0) return Number.MAX_SAFE_INTEGER;
  return relevant.reduce((sum, v) => sum + v, 0) / relevant.length;
}
