import type { Edge, Node } from "@xyflow/react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { AudioDevice } from "../lib/api";
import type { AudiorouterConfig, DeviceConfig, RouteConfig } from "../types";
import type { DeviceNodeData, RouteEdgeData } from "./flow-types";

export type Selection =
  | { kind: "none" }
  | { kind: "device"; id: string }
  | { kind: "edge"; id: string };

interface Props {
  selection: Selection;
  nodes: Node[];
  edges: Edge[];
  config: AudiorouterConfig;
  onUpdateDevice: (id: string, patch: Partial<DeviceConfig>) => void;
  onUpdateRoute: (id: string, patch: Partial<RouteConfig>) => void;
  onDeleteDevice: (id: string) => void;
  onDeleteRoute: (id: string) => void;
  onAddDevice: () => void;
  /** Known CoreAudio devices for autocomplete suggestions. */
  availableDevices: AudioDevice[];
  /** When true (canvas locked), all editing controls are disabled. */
  readOnly?: boolean;
  /** When true, config is still loading — disable the add button. */
  loading?: boolean;
}

export function SidePanel({
  selection,
  nodes,
  edges,
  onUpdateDevice,
  onUpdateRoute,
  onDeleteDevice,
  onDeleteRoute,
  onAddDevice,
  availableDevices,
  readOnly = false,
  loading = false,
}: Props) {
  if (selection.kind === "none") {
    return (
      <div className="flex h-full flex-col p-4">
        <h2 className="mb-4 text-sm font-semibold text-[var(--color-foreground)]">Inspector</h2>
        {readOnly && <ReadOnlyBanner />}
        <div className="flex flex-1 flex-col items-center justify-center text-center">
          <p className="mb-6 text-xs text-[var(--color-muted-foreground)]">
            ノードまたはエッジを選択して編集
          </p>
          <button
            type="button"
            onClick={onAddDevice}
            disabled={readOnly || loading}
            className="rounded-md border border-[var(--color-border)] bg-[var(--color-secondary)] px-4 py-2 text-sm font-medium text-[var(--color-secondary-foreground)] transition hover:bg-[var(--color-muted)] disabled:opacity-40 disabled:cursor-not-allowed"
          >
            + デバイス追加
          </button>
        </div>
      </div>
    );
  }

  if (selection.kind === "device") {
    const node = nodes.find((n) => n.id === selection.id);
    if (!node) return null;
    const data = node.data as DeviceNodeData;

    const usedDeviceNames = new Set(
      nodes
        .filter((n) => n.id !== selection.id)
        .map((n) => (n.data as DeviceNodeData).device)
        .filter(Boolean),
    );

    return (
      <div className="flex h-full flex-col p-4">
        <h2 className="mb-4 text-sm font-semibold text-[var(--color-foreground)]">デバイス設定</h2>
        {readOnly && <ReadOnlyBanner />}

        <div className="space-y-4">
          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-[var(--color-muted-foreground)]">
              エイリアス名（省略可）
            </span>
            <input
              type="text"
              value={data.name}
              placeholder={data.device || "デバイス名と同じ"}
              autoComplete="off"
              disabled={readOnly}
              onChange={(e) => onUpdateDevice(selection.id, { name: e.target.value })}
              className="w-full rounded-md border border-[var(--color-input)] bg-[var(--color-background)] px-3 py-2 text-sm text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] focus:ring-1 focus:ring-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed"
            />
          </label>

          <label className="block">
            <span className="mb-1.5 block text-xs font-medium text-[var(--color-muted-foreground)]">
              CoreAudio デバイス名
            </span>
            <DeviceNameCombobox
              value={data.device}
              disabled={readOnly}
              hasMissingDevice={data.missingInput || data.missingOutput}
              availableDevices={availableDevices}
              usedDeviceNames={usedDeviceNames}
              onChange={(device) => onUpdateDevice(selection.id, { device })}
            />
            {(data.missingInput || data.missingOutput) && (
              <span
                className="mt-1 block text-xs italic"
                style={{ color: "var(--color-ar-disabled)" }}
              >
                {"⚠ デバイスが見つかりません"}
              </span>
            )}
          </label>

          {/* Channel summary (mirrors tui.rs top border ▲/▼) */}
          <div className="rounded-md border border-[var(--color-border)] bg-[var(--color-muted)] p-3">
            <span className="mb-2 block text-xs font-medium text-[var(--color-muted-foreground)]">
              ルーティング情報
            </span>
            <div className="flex items-center gap-3 font-mono text-xs">
              <span style={{ color: "var(--color-ar-in)" }}>
                ▲ {data.channels.chIn}
                {data.channels.totalIn > 0 ? `/${data.channels.totalIn}` : ""}
              </span>
              <span style={{ color: "var(--color-ar-out)" }}>
                ▼ {data.channels.chOut}
                {data.channels.totalOut > 0 ? `/${data.channels.totalOut}` : ""}
              </span>
            </div>
          </div>

          {/* Limiter toggle (mirrors tui.rs 🧱 indicator) */}
          <div>
            <span className="mb-1.5 block text-xs font-medium text-[var(--color-muted-foreground)]">
              リミッター（出力時）
            </span>
            <TogglePill
              on={data.limiter}
              onLabel="有効"
              offLabel="無効"
              activeColor="var(--color-ar-gain)"
              disabled={readOnly}
              onChange={() => onUpdateDevice(selection.id, { limiter: !data.limiter })}
            />
          </div>
        </div>

        <div className="mt-auto pt-6">
          <button
            type="button"
            disabled={readOnly}
            onClick={() => onDeleteDevice(selection.id)}
            className="w-full rounded-md border border-[var(--color-destructive)] py-2 text-sm text-[var(--color-destructive)] transition hover:bg-[color-mix(in_oklch,var(--color-destructive)_10%,transparent)] disabled:opacity-40 disabled:cursor-not-allowed"
          >
            デバイスを削除
          </button>
        </div>
      </div>
    );
  }

  // edge (route) selected
  const edge = edges.find((e) => e.id === selection.id);
  if (!edge) return null;
  const data = edge.data as RouteEdgeData;

  // Resolve source/dest nodes to determine max channels and missing state
  const fromNode = nodes.find((n) => {
    const d = n.data as DeviceNodeData;
    return (d.name || d.device) === data.from;
  });
  const toNode = nodes.find((n) => {
    const d = n.data as DeviceNodeData;
    return (d.name || d.device) === data.to;
  });
  const fromNodeData = fromNode?.data as DeviceNodeData | undefined;
  const toNodeData = toNode?.data as DeviceNodeData | undefined;
  // 0 means unknown (missing device or not yet resolved)
  const fromMaxCh = fromNodeData?.channels.totalIn ?? 0;
  const toMaxCh = toNodeData?.channels.totalOut ?? 0;

  const dim = data.disabled || data.mute;

  const pairCount = Math.max(data.from_channels.length, data.to_channels.length, 1);
  const pairs = Array.from({ length: pairCount }, (_, i) => ({
    from: data.from_channels[i] ?? 1,
    to: data.to_channels[i] ?? 1,
  }));

  const updatePair = (index: number, side: "from" | "to", value: number) => {
    const newFrom = data.from_channels.slice();
    const newTo = data.to_channels.slice();
    while (newFrom.length < pairCount) newFrom.push(1);
    while (newTo.length < pairCount) newTo.push(1);
    if (side === "from") newFrom[index] = value;
    else newTo[index] = value;
    onUpdateRoute(selection.id, { from_channels: newFrom, to_channels: newTo });
  };

  const addPair = () => {
    const lastFrom = data.from_channels.at(-1) ?? 0;
    const lastTo = data.to_channels.at(-1) ?? 0;
    const nextFrom = fromMaxCh > 0 ? Math.min(lastFrom + 1, fromMaxCh) : lastFrom + 1;
    const nextTo = toMaxCh > 0 ? Math.min(lastTo + 1, toMaxCh) : lastTo + 1;
    onUpdateRoute(selection.id, {
      from_channels: [...data.from_channels, Math.max(1, nextFrom)],
      to_channels: [...data.to_channels, Math.max(1, nextTo)],
    });
  };

  const removePair = (index: number) => {
    if (data.from_channels.length <= 1) return;
    onUpdateRoute(selection.id, {
      from_channels: data.from_channels.filter((_, i) => i !== index),
      to_channels: data.to_channels.filter((_, i) => i !== index),
    });
  };

  return (
    <div className="flex h-full flex-col p-4">
      <div className="mb-4 flex items-center gap-2">
        <h2 className="text-sm font-semibold text-[var(--color-foreground)]">ルート設定</h2>
        <span className="text-xs text-[var(--color-muted-foreground)]">
          {data.from} → {data.to}
        </span>
      </div>
      {readOnly && <ReadOnlyBanner />}

      <div className="space-y-4">
        {/* Channel pair editor */}
        <div>
          <div className="mb-1.5 grid grid-cols-[1fr_1fr_1fr] gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--color-ar-in)" }}>
              from ch{fromMaxCh > 0 ? ` (1–${fromMaxCh})` : ""}
            </span>
            <span />
            <span className="text-xs font-medium" style={{ color: "var(--color-ar-out)" }}>
              to ch{toMaxCh > 0 ? ` (1–${toMaxCh})` : ""}
            </span>
          </div>
          <div className="space-y-1">
            {pairs.map((pair, i) => (
              <div key={i} className="flex items-center gap-1.5">
                <div className="grid flex-1 grid-cols-[1fr_1fr_1fr] items-center gap-1.5">
                  <ChannelInput
                    value={pair.from}
                    min={1}
                    max={fromMaxCh > 0 ? fromMaxCh : undefined}
                    disabled={readOnly}
                    onChange={(v) => updatePair(i, "from", v)}
                    className="w-full rounded border border-[var(--color-input)] bg-[var(--color-background)] px-2 py-1 text-center font-mono text-sm text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] focus:ring-1 focus:ring-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed"
                  />
                  {/* Connecting line — mirrors unselected route edge style */}
                  <div className="flex items-center gap-0">
                    <div
                      className="flex-1"
                      style={
                        dim
                          ? {
                              height: "2px",
                              backgroundImage:
                                "repeating-linear-gradient(90deg, var(--color-ar-disabled) 0, var(--color-ar-disabled) 5px, transparent 5px, transparent 8px)",
                              backgroundSize: "8px 2px",
                            }
                          : {
                              height: "2px",
                              backgroundImage:
                                "repeating-linear-gradient(90deg, var(--color-ar-route) 0, var(--color-ar-route) 5px, transparent 5px, transparent 8px)",
                              backgroundSize: "8px 2px",
                              animation: "ar-flow-right 0.4s linear infinite",
                            }
                      }
                    />
                    <svg
                      width="7"
                      height="10"
                      viewBox="0 0 7 10"
                      style={{ flexShrink: 0, display: "block" }}
                    >
                      <polygon
                        points="0,0 7,5 0,10"
                        fill={dim ? "var(--color-ar-disabled)" : "var(--color-ar-route)"}
                      />
                    </svg>
                  </div>
                  <ChannelInput
                    value={pair.to}
                    min={1}
                    max={toMaxCh > 0 ? toMaxCh : undefined}
                    disabled={readOnly}
                    onChange={(v) => updatePair(i, "to", v)}
                    className="w-full rounded border border-[var(--color-input)] bg-[var(--color-background)] px-2 py-1 text-center font-mono text-sm text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] focus:ring-1 focus:ring-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed"
                  />
                </div>
                <button
                  type="button"
                  onClick={() => removePair(i)}
                  disabled={pairs.length <= 1 || readOnly}
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-sm text-[var(--color-foreground)]/60 transition hover:text-[var(--color-destructive)] disabled:opacity-20 disabled:cursor-not-allowed"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          <button
            type="button"
            disabled={readOnly}
            onClick={addPair}
            className="mt-2 w-full rounded border border-dashed border-[var(--color-border)] py-1 text-xs text-[var(--color-muted-foreground)] transition hover:border-[var(--color-ring)] hover:text-[var(--color-foreground)] disabled:opacity-40 disabled:cursor-not-allowed"
          >
            + 行追加
          </button>
        </div>

        {/* Gain slider (mirrors tui.rs gain label formatting) */}
        <div>
          <span className="mb-1.5 block text-xs font-medium text-[var(--color-muted-foreground)]">
            ゲイン
            <span
              className="ml-2 font-mono font-semibold"
              style={{ color: "var(--color-ar-gain)" }}
            >
              {data.gain_db > 0 ? "+" : ""}
              {data.gain_db.toFixed(1)} dB
            </span>
          </span>
          <div className="flex items-center gap-3">
            <input
              type="range"
              min={-60}
              max={12}
              step={0.5}
              value={data.gain_db}
              disabled={readOnly}
              onChange={(e) =>
                onUpdateRoute(selection.id, {
                  gain_db: Number(e.target.value),
                })
              }
              className="flex-1 disabled:opacity-50 disabled:cursor-not-allowed"
              style={{ accentColor: "var(--color-ar-gain)" }}
            />
            <input
              type="number"
              value={data.gain_db}
              step={0.5}
              disabled={readOnly}
              onChange={(e) =>
                onUpdateRoute(selection.id, {
                  gain_db: Number(e.target.value),
                })
              }
              className="w-16 rounded-md border border-[var(--color-input)] bg-[var(--color-background)] px-2 py-1 text-sm text-[var(--color-foreground)] outline-none disabled:opacity-50 disabled:cursor-not-allowed"
            />
          </div>
        </div>

        {/* Mute toggle (mirrors tui.rs mute X indicator) */}
        <div>
          <span className="mb-1.5 block text-xs font-medium text-[var(--color-muted-foreground)]">
            ミュート
          </span>
          <TogglePill
            on={data.mute}
            onLabel="ミュート中"
            offLabel="オフ"
            activeColor="var(--color-ar-clip)"
            disabled={readOnly}
            onChange={() => onUpdateRoute(selection.id, { mute: !data.mute })}
          />
        </div>
      </div>

      <div className="mt-auto pt-6">
        <button
          type="button"
          disabled={readOnly}
          onClick={() => onDeleteRoute(selection.id)}
          className="w-full rounded-md border border-[var(--color-destructive)] py-2 text-sm text-[var(--color-destructive)] transition hover:bg-[color-mix(in_oklch,var(--color-destructive)_10%,transparent)] disabled:opacity-40 disabled:cursor-not-allowed"
        >
          ルートを削除
        </button>
      </div>
    </div>
  );
}

function DeviceNameCombobox({
  value,
  disabled,
  hasMissingDevice,
  availableDevices,
  usedDeviceNames,
  onChange,
}: {
  value: string;
  disabled?: boolean;
  hasMissingDevice: boolean;
  availableDevices: AudioDevice[];
  usedDeviceNames: Set<string>;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);

  const suggestions = useMemo(() => {
    const q = value.trim().toLocaleLowerCase();
    return availableDevices
      .filter((dev) => !usedDeviceNames.has(dev.name) || dev.name === value)
      .filter((dev) => q === "" || dev.name.toLocaleLowerCase().includes(q))
      .slice(0, 8);
  }, [availableDevices, usedDeviceNames, value]);

  useEffect(() => {
    setActiveIndex(0);
  }, [suggestions.length, value]);

  const selectSuggestion = (name: string) => {
    onChange(name);
    setOpen(false);
  };

  const showMenu = open && !disabled && suggestions.length > 0;

  return (
    <div className="relative">
      <input
        type="text"
        value={value}
        placeholder="例: VT-4"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        disabled={disabled}
        role="combobox"
        aria-expanded={showMenu}
        aria-autocomplete="list"
        aria-controls="coreaudio-device-suggestions"
        onFocus={() => setOpen(true)}
        onBlur={() => window.setTimeout(() => setOpen(false), 100)}
        onChange={(e) => {
          onChange(e.target.value);
          setOpen(true);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setOpen(true);
            setActiveIndex((i) => Math.min(i + 1, suggestions.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActiveIndex((i) => Math.max(i - 1, 0));
          } else if (e.key === "Enter" && showMenu) {
            e.preventDefault();
            selectSuggestion(suggestions[activeIndex]?.name ?? suggestions[0].name);
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
        className={`w-full rounded-md border bg-[var(--color-background)] px-3 py-2 text-sm text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] focus:ring-1 focus:ring-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed ${
          hasMissingDevice ? "border-[var(--color-ar-disabled)]" : "border-[var(--color-input)]"
        }`}
      />
      {showMenu && (
        <div
          id="coreaudio-device-suggestions"
          role="listbox"
          className="absolute z-50 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-[var(--color-border)] bg-[var(--color-card)] py-1 text-sm text-[var(--color-foreground)] shadow-lg shadow-black/40"
        >
          {suggestions.map((dev, index) => {
            const selected = index === activeIndex;
            return (
              <button
                key={dev.name}
                type="button"
                role="option"
                aria-selected={selected}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => selectSuggestion(dev.name)}
                className="flex w-full items-center justify-between gap-3 px-3 py-2 text-left transition"
                style={{
                  background: selected ? "var(--color-muted)" : "transparent",
                  color: "var(--color-foreground)",
                }}
              >
                <span className="min-w-0 truncate">{dev.name}</span>
                <span className="shrink-0 text-[10px] text-[var(--color-muted-foreground)]">
                  {deviceKindLabel(dev)}
                  {dev.isDefaultInput ? " · default in" : ""}
                  {dev.isDefaultOutput ? " · default out" : ""}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function deviceKindLabel(dev: AudioDevice): string {
  if (dev.maxInputChannels > 0 && dev.maxOutputChannels > 0) return "in/out";
  if (dev.maxInputChannels > 0) return "input";
  return "output";
}

function TogglePill({
  on,
  onLabel,
  offLabel,
  activeColor,
  disabled,
  onChange,
}: {
  on: boolean;
  onLabel: string;
  offLabel: string;
  activeColor: string;
  disabled?: boolean;
  onChange: () => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      disabled={disabled}
      onClick={onChange}
      className="flex w-full items-center gap-2.5 rounded-md border px-3 py-2 text-sm transition duration-150 disabled:cursor-not-allowed disabled:opacity-40"
      style={{
        borderColor: on ? activeColor : "var(--color-border)",
        background: on ? `color-mix(in oklch, ${activeColor} 12%, transparent)` : "transparent",
        color: on ? activeColor : "var(--color-muted-foreground)",
      }}
    >
      {/* Track */}
      <div
        className="relative h-4 w-7 shrink-0 rounded-full transition-colors duration-150"
        style={{ background: on ? activeColor : "var(--color-muted)" }}
      >
        {/* Thumb */}
        <div
          className="absolute top-0.5 h-3 w-3 rounded-full bg-white shadow-sm transition-transform duration-150"
          style={{ transform: on ? "translateX(12px)" : "translateX(2px)" }}
        />
      </div>
      <span className="text-xs font-medium">{on ? onLabel : offLabel}</span>
    </button>
  );
}

function ReadOnlyBanner() {
  return (
    <div
      className="mb-3 flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-xs"
      style={{
        borderColor: "var(--color-ar-disabled)",
        color: "var(--color-muted-foreground)",
        background: "color-mix(in oklch, var(--color-ar-disabled) 10%, transparent)",
      }}
    >
      <span>🔒</span>
      <span>ロック中 — 編集するにはToggle Interactiveをオフにしてください</span>
    </div>
  );
}

/**
 * Number input that allows free editing (including clearing) while the user
 * types, and normalizes to a valid value only on blur. This avoids the
 * "snaps back on backspace" problem with purely controlled number inputs.
 */
function ChannelInput({
  value,
  min,
  max,
  disabled,
  onChange,
  className,
}: {
  value: number;
  min: number;
  max?: number;
  disabled?: boolean;
  onChange: (value: number) => void;
  className?: string;
}) {
  const [local, setLocal] = useState(String(value));
  const externalRef = useRef(value);

  useEffect(() => {
    if (externalRef.current !== value) {
      externalRef.current = value;
      setLocal(String(value));
    }
  }, [value]);

  const commit = (raw: string) => {
    const n = Number.parseInt(raw, 10);
    const clamped = !Number.isFinite(n) || n < min ? min : max !== undefined && n > max ? max : n;
    externalRef.current = clamped;
    setLocal(String(clamped));
    onChange(clamped);
  };

  return (
    <input
      type="number"
      min={min}
      max={max}
      value={local}
      disabled={disabled}
      onChange={(e) => setLocal(e.target.value)}
      onBlur={(e) => commit(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
      }}
      className={className}
    />
  );
}
