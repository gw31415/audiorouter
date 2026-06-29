import { BaseEdge, EdgeLabelRenderer, useEdges, useNodes, type EdgeProps } from "@xyflow/react";
import { useCallback, useMemo, useRef } from "react";
import type { RouteEdgeData } from "./flow-types";
import { useRouteUpdate } from "./route-update-context";

const STEP = 24;
const LABEL_MIN_SEP = 20;

/**
 * 1-D overlap resolution with centroid preservation.
 *
 * Items are pushed apart (forward + backward pass) until all neighbours
 * satisfy `minSep`, then the whole group is re-centred around its
 * original mean so it does not drift.
 */
function resolveOffsets(desired: number[], minSep: number): number[] {
  if (desired.length <= 1) return [...desired];
  const items = desired.map((y, i) => ({ y, i }));
  items.sort((a, b) => a.y - b.y);
  for (let k = 1; k < items.length; k++) {
    if (items[k].y - items[k - 1].y < minSep) items[k].y = items[k - 1].y + minSep;
  }
  for (let k = items.length - 2; k >= 0; k--) {
    if (items[k + 1].y - items[k].y < minSep) items[k].y = items[k + 1].y - minSep;
  }
  const origMean = desired.reduce((s, y) => s + y, 0) / desired.length;
  const newMean = items.reduce((s, p) => s + p.y, 0) / items.length;
  const shift = origMean - newMean;
  items.forEach((p) => {
    p.y += shift;
  });
  const result = [...desired];
  items.forEach((p) => {
    result[p.i] = p.y;
  });
  return result;
}

export function RouteEdge({
  id,
  source,
  target,
  sourceX,
  sourceY,
  targetX,
  targetY,
  data,
  selected,
}: EdgeProps) {
  const d = data as RouteEdgeData;
  const disabled = d?.disabled ?? false;
  const mute = d?.mute ?? false;
  const dim = disabled || mute;

  const parallelCount = Math.max(1, d?.parallelCount ?? 1);
  const parallelIndex = d?.parallelIndex ?? 0;
  const parallelOffset = (parallelIndex - (parallelCount - 1) / 2) * STEP;

  // ── Path geometry ──────────────────────────────────────────────────
  //
  // Single cubic bezier from handle to handle.
  // Three-segment paths cause G2 discontinuities (visible kinks) at the
  // junction points even when G1-continuous.  A single bezier distributes
  // the vertical movement of parallelOffset evenly over the full span so
  // the curve stays smooth everywhere.
  //
  // Control-point horizontal reach = 50 % of span (standard S-curve).
  // Control-point Y is shifted by parallelOffset so parallel routes on the
  // same pair arc away from each other in the middle.
  // Both handles are at the exact node handle coordinates. ✓
  const span = targetX - sourceX;
  const cpX = Math.abs(span) * 0.5;
  const edgePath = [
    `M ${sourceX} ${sourceY}`,
    `C ${sourceX + cpX} ${sourceY + parallelOffset}`,
    `  ${targetX - cpX} ${targetY + parallelOffset}`,
    `  ${targetX} ${targetY}`,
  ].join(" ");

  // Exact bezier midpoint at t = 0.5:
  //   x = ⅛·P0 + ⅜·CP1 + ⅜·CP2 + ⅛·P3
  //     = (sourceX + targetX) / 2  (symmetric control points)
  //   y = (sourceY + targetY) / 2 + 0.75 · parallelOffset
  const gainLabelX = (sourceX + targetX) / 2;
  const gainLabelY = (sourceY + targetY) / 2 + parallelOffset * 0.75;

  // Channel label X: small fixed inset from each handle (capped so labels
  // stay on the correct side for very short edges).
  const labelInset = Math.min(44, Math.abs(span) * 0.2);
  const srcLabelX = sourceX + labelInset;
  const tgtLabelX = targetX - labelInset;

  // ── Label repulsion ────────────────────────────────────────────────
  //
  // Sort siblings by the PEER node's Y position so label order always
  // matches the visual path order.  useNodes() re-fires on every drag so
  // labels re-order in real-time.
  const allEdges = useEdges();
  const allNodes = useNodes();

  const nodeYMap = useMemo(() => new Map(allNodes.map((n) => [n.id, n.position.y])), [allNodes]);

  // Source-side: sort by target node Y
  const sameSource = useMemo(
    () =>
      allEdges
        .filter((e) => e.source === source)
        .sort((a, b) => (nodeYMap.get(a.target) ?? 0) - (nodeYMap.get(b.target) ?? 0)),
    [allEdges, source, nodeYMap],
  );
  const srcRank = sameSource.findIndex((e) => e.id === id);
  const srcLabelOffset = useMemo(() => {
    const n = sameSource.length;
    const desired = sameSource.map((_, k) => (k - (n - 1) / 2) * STEP);
    return resolveOffsets(desired, LABEL_MIN_SEP)[srcRank] ?? 0;
  }, [sameSource, srcRank]);

  // Destination-side: sort by source node Y
  const sameDest = useMemo(
    () =>
      allEdges
        .filter((e) => e.target === target)
        .sort((a, b) => (nodeYMap.get(a.source) ?? 0) - (nodeYMap.get(b.source) ?? 0)),
    [allEdges, target, nodeYMap],
  );
  const dstRank = sameDest.findIndex((e) => e.id === id);
  const dstLabelOffset = useMemo(() => {
    const n = sameDest.length;
    const desired = sameDest.map((_, k) => (k - (n - 1) / 2) * STEP);
    return resolveOffsets(desired, LABEL_MIN_SEP)[dstRank] ?? 0;
  }, [sameDest, dstRank]);

  // ── Arrowhead at target end ────────────────────────────────────────
  //
  // SVG marker attached to the BaseEdge path so it inherits the same
  // z-index layer as the path (fixes overlap ordering with dimmed edges).
  // orient="auto" rotates the marker to match the path tangent at t=1.
  // userSpaceOnUse keeps pixel dimensions stable across zoom levels.
  const arrowLen = selected ? 13 : 9;
  const arrowHalf = selected ? 6.5 : 4.5;
  const markerId = `arrow-${id}`;

  // ── Colors ─────────────────────────────────────────────────────────
  // Dim variants: color-mix blends the original hue with the background
  // to produce an opaque faded color that doesn't bleed underlying objects.
  const dimRoute = "color-mix(in oklch, var(--color-ar-route) 40%, var(--color-background))";
  const dimIn = "color-mix(in oklch, var(--color-ar-in)    40%, var(--color-card))";
  const dimOut = "color-mix(in oklch, var(--color-ar-out)   40%, var(--color-card))";
  const dimGain = "color-mix(in oklch, var(--color-ar-gain)  40%, var(--color-card))";

  const selectedColor = dim ? "var(--color-ring-dim)" : "var(--color-ring)";
  const strokeColor = selected ? selectedColor : dim ? dimRoute : "var(--color-ar-route)";
  const strokeWidth = selected ? 4.5 : dim ? 1.5 : 2;
  const dashArray = dim ? "5 3" : undefined;

  const gainLabel = disabled
    ? "OFF"
    : mute
      ? "✕"
      : d.gain_db === 0
        ? "0dB"
        : `${d.gain_db > 0 ? "+" : ""}${d.gain_db.toFixed(1)}dB`;

  const srcLabel = d ? channelFmt(d.from_channels) : "";
  const dstLabel = d ? channelFmt(d.to_channels) : "";

  return (
    <>
      <defs>
        <marker
          id={markerId}
          markerWidth={arrowLen}
          markerHeight={arrowHalf * 2}
          refX={arrowLen}
          refY={arrowHalf}
          orient="auto"
          markerUnits="userSpaceOnUse"
        >
          <polygon
            points={`0 0, ${arrowLen} ${arrowHalf}, 0 ${arrowHalf * 2}`}
            fill={strokeColor}
          />
        </marker>
      </defs>
      <BaseEdge
        id={id}
        path={edgePath}
        interactionWidth={30}
        markerEnd={`url(#${markerId})`}
        style={{
          stroke: strokeColor,
          strokeWidth,
          strokeDasharray: dashArray,
        }}
      />
      <EdgeLabelRenderer>
        {/* Source channel label */}
        {srcLabel && (
          <div
            className="nodrag nopan absolute rounded px-1 font-mono text-[10px]"
            style={{
              transform: `translate(-50%, -50%) translate(${srcLabelX}px, ${sourceY + srcLabelOffset}px)`,
              pointerEvents: "none",
              color: selected ? selectedColor : dim ? dimIn : "var(--color-ar-in)",
              background: "var(--color-card)",
              fontWeight: 500,
            }}
          >
            {srcLabel}
          </div>
        )}

        {/* Gain control — minimal label at bezier midpoint.
            Click = toggle mute, vertical drag = adjust gain. */}
        <GainLabel
          x={gainLabelX}
          y={gainLabelY}
          label={gainLabel}
          gainDb={d.gain_db}
          mute={mute}
          disabled={disabled}
          selected={selected ?? false}
          selectedColor={selectedColor}
          dimGain={dimGain}
          routeColor={strokeColor}
          edgeId={id}
        />

        {/* Destination channel label */}
        {dstLabel && (
          <div
            className="nodrag nopan absolute rounded px-1 font-mono text-[10px]"
            style={{
              transform: `translate(-50%, -50%) translate(${tgtLabelX}px, ${targetY + dstLabelOffset}px)`,
              pointerEvents: "none",
              color: selected ? selectedColor : dim ? dimOut : "var(--color-ar-out)",
              background: "var(--color-card)",
              fontWeight: 500,
            }}
          >
            {dstLabel}
          </div>
        )}
      </EdgeLabelRenderer>
    </>
  );
}

function channelFmt(channels: number[]): string {
  return channels.join(",");
}

// ── GainLabel ──────────────────────────────────────────────────────
//
// A minimal label at the bezier midpoint — same visual style as the
// channel labels, but interactive.
//
//   click         → toggle mute
//   drag ↕        → adjust gain (-20 … +20 dB, 0.5 dB steps)
//   wheel         → fine-adjust gain (1 dB steps)
//
// Click vs drag is disambiguated by a 3px movement threshold.

const GAIN_MIN = -20;
const GAIN_MAX = 20;
const DRAG_THRESHOLD = 3;

interface GainLabelProps {
  x: number;
  y: number;
  label: string;
  gainDb: number;
  mute: boolean;
  disabled: boolean;
  selected: boolean;
  selectedColor: string;
  dimGain: string;
  routeColor: string;
  edgeId: string;
}

function GainLabel({
  x,
  y,
  label,
  gainDb,
  mute,
  disabled,
  selected,
  selectedColor,
  dimGain,
  routeColor,
  edgeId,
}: GainLabelProps) {
  const update = useRouteUpdate();
  const dragRef = useRef<{ startY: number; startGain: number; moved: boolean } | null>(null);

  const clampGain = (v: number) => Math.max(GAIN_MIN, Math.min(GAIN_MAX, v));

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (disabled) return;
      e.stopPropagation();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      dragRef.current = { startY: e.clientY, startGain: gainDb, moved: false };
    },
    [disabled, gainDb],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      const s = dragRef.current;
      if (!s) return;
      // Up = increase, down = decrease
      const dy = s.startY - e.clientY;
      if (Math.abs(dy) > DRAG_THRESHOLD) s.moved = true;
      if (s.moved && update) {
        // 1px ≈ 0.25 dB, snapped to 0.5
        const raw = s.startGain + dy * 0.25;
        update(edgeId, { gain_db: Math.round(clampGain(raw) * 2) / 2 });
      }
    },
    [edgeId, update],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent) => {
      const s = dragRef.current;
      dragRef.current = null;
      try {
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
      } catch {
        // already released
      }
      if (s && !s.moved && update && !disabled) {
        update(edgeId, { mute: !mute });
      }
    },
    [edgeId, mute, disabled, update],
  );

  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      if (disabled || !update) return;
      e.stopPropagation();
      const step = e.shiftKey ? 0.5 : 1;
      update(edgeId, { gain_db: clampGain(gainDb + (e.deltaY < 0 ? step : -step)) });
    },
    [edgeId, gainDb, disabled, update],
  );

  const dim = disabled || mute;
  const textColor = disabled
    ? "var(--color-ar-disabled)"
    : mute
      ? "var(--color-ar-clip)"
      : selected
        ? selectedColor
        : dim
          ? dimGain
          : "var(--color-ar-gain)";

  // 0 dB + not mute + not disabled → show a dot in the route color
  const isDot = gainDb === 0 && !mute && !disabled;
  // Dot uses the same color as the edge path
  const dotColor = selected ? selectedColor : dim ? dimGain : routeColor;

  return (
    <div
      className="nodrag nopan absolute cursor-pointer"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onWheel={onWheel}
      style={{
        transform: `translate(-50%, -50%) translate(${x}px, ${y}px)`,
        pointerEvents: disabled ? "none" : "auto",
        userSelect: "none",
      }}
    >
      {isDot ? (
        <div className="rounded-full" style={{ width: 8, height: 8, background: dotColor }} />
      ) : (
        <div
          className="rounded px-1.5 py-0.5 font-mono text-[11px]"
          style={{
            color: textColor,
            background: "var(--color-card)",
            fontWeight: 600,
          }}
        >
          {label}
        </div>
      )}
    </div>
  );
}
