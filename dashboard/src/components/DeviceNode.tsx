import { Handle, Position, type NodeProps } from "@xyflow/react";
import type { DeviceNodeData } from "./flow-types";

/**
 * Device node rendered in the React Flow canvas.
 *
 * Design mirrors `tui.rs::draw_device_node`:
 *
 * ┌────────────────────────────┐
 * │ ▲2/4 ▼2/2                  │  ← channel info on top border
 * ╞════════════════════════════╡
 * │ alias                 🧱 │  ← name + limiter indicator
 * │ device-name                │
 * │                            │
 * ╰────────────────────────────╯
 *
 * When the device is missing (not connected), the entire node is dimmed
 * and a "device missing" message replaces the body — mirroring tui.rs
 * `unavailable` handling.
 *
 * Color semantics (from tui.rs):
 *   cyan   = border (available)
 *   dim    = unavailable
 */
export function DeviceNode({ data, selected }: NodeProps) {
  const d = data as unknown as DeviceNodeData;

  const unavailable = d.missingInput || d.missingOutput;

  const borderColor = unavailable
    ? "var(--color-ar-disabled)"
    : selected
      ? "var(--color-ring)"
      : "var(--color-ar-border)";

  // Channel badges on the "border" row — mirrors tui.rs top border overlay.
  // Normal devices show used/total, but omit a side with no channels at all
  // (e.g. ▲2/2, not ▲2/2 ▼0/0). Missing devices keep routing info visible
  // without denominators (e.g. ▲2 ▼0), because total hardware channels are
  // unknown/unavailable.
  const upStr = unavailable
    ? `▲${d.channels.chIn}`
    : d.channels.totalIn > 0
      ? `▲${d.channels.chIn}/${d.channels.totalIn}`
      : d.channels.chIn > 0
        ? `▲${d.channels.chIn}`
        : null;
  const downStr = unavailable
    ? `▼${d.channels.chOut}`
    : d.channels.totalOut > 0
      ? `▼${d.channels.chOut}/${d.channels.totalOut}`
      : d.channels.chOut > 0
        ? `▼${d.channels.chOut}`
        : null;

  const upDim = d.channels.chIn === 0;
  const downDim = d.channels.chOut === 0;

  // Dim variants: color-mix blends each original hue with the card background
  // to produce an opaque faded color without any transparency.
  const dimOf = (c: string) => `color-mix(in oklch, ${c} 40%, var(--color-card))`;

  const channelUpColor = unavailable || upDim ? dimOf("var(--color-ar-in)") : "var(--color-ar-in)";
  const channelDownColor =
    unavailable || downDim ? dimOf("var(--color-ar-out)") : "var(--color-ar-out)";
  const nameFgColor = unavailable ? dimOf("var(--color-foreground)") : "var(--color-foreground)";
  const limiterColor = unavailable ? dimOf("var(--color-ar-gain)") : "var(--color-ar-gain)";

  // React Flow connection handles correspond to route roles:
  // - source/right "out" handle = this device can be route.from (input/capture)
  // - target/left  "in"  handle = this device can be route.to (output/playback)
  // For missing devices, always show both handles: channel count is unknown so
  // we allow the user to connect routes in either direction for configuration.
  const showInputHandle = unavailable || d.channels.totalIn > 0 || d.channels.chIn > 0;
  const showOutputHandle = unavailable || d.channels.totalOut > 0 || d.channels.chOut > 0;

  // Missing label — shown when the device is not connected
  const missingLabel = unavailable ? "(device missing)" : null;

  return (
    <div
      className="relative w-[220px] rounded-lg bg-[var(--color-card)] shadow-lg transition-all duration-150"
      style={{
        borderWidth: "1px",
        borderStyle: "solid",
        borderColor,
        boxShadow: selected
          ? "0 0 14px var(--color-ring-glow), 0 0 36px var(--color-ring-glow-far)"
          : "0 2px 8px rgba(0,0,0,0.3)",
      }}
    >
      {/* Output/playback target handle (left) — route.to */}
      {showOutputHandle && (
        <Handle
          id="in"
          type="target"
          position={Position.Left}
          className="!h-2.5 !w-2.5 !rounded-full !border-2 !border-[var(--color-background)] !bg-[var(--color-ar-out)]"
        />
      )}

      {/* ── Channel info bar (top border) ─────────────────── */}
      {/* Mirrors tui.rs: channel info overlaid on top border */}
      <div className="flex min-h-[25px] items-center gap-2 border-b border-[var(--color-border)] px-3 py-1">
        {upStr && (
          <span className="font-mono text-[10px] font-semibold" style={{ color: channelUpColor }}>
            {upStr}
          </span>
        )}
        {upStr && downStr && <span className="text-[10px] text-[var(--color-border)]">·</span>}
        {downStr && (
          <span className="font-mono text-[10px] font-semibold" style={{ color: channelDownColor }}>
            {downStr}
          </span>
        )}
      </div>

      {/* ── Node body ─────────────────────────────────────── */}
      <div className="flex items-center gap-2 px-3 py-2.5">
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold" style={{ color: nameFgColor }}>
            {/* Show effective name (alias if set, else device name) */}
            {d.name || d.device}
          </div>
          {(() => {
            const subtitle = missingLabel ?? (d.name && d.name !== d.device ? d.device : null);
            return (
              <div
                className={`truncate text-xs${subtitle ? "" : " invisible"}`}
                style={
                  missingLabel
                    ? { color: "var(--color-ar-disabled)", fontStyle: "italic" }
                    : { color: "var(--color-muted-foreground)" }
                }
              >
                {subtitle ?? " "}
              </div>
            );
          })()}
        </div>

        {/* Right-aligned indicators (mirrors tui.rs title line) */}
        <div className="flex items-center gap-1">
          {d.limiter && (
            <span title="Limiter active" className="text-sm" style={{ color: limiterColor }}>
              🧱
            </span>
          )}
        </div>
      </div>

      {/* Input/capture source handle (right) — route.from */}
      {showInputHandle && (
        <Handle
          id="out"
          type="source"
          position={Position.Right}
          className="!h-2.5 !w-2.5 !rounded-full !border-2 !border-[var(--color-background)] !bg-[var(--color-ar-in)]"
        />
      )}
    </div>
  );
}
