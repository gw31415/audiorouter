import {
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EngineBar } from "./components/EngineBar";
import type { DeviceNodeData, RouteEdgeData } from "./components/flow-types";
import {
  applyAutoLayout,
  configToFlow,
  createRouteEdge,
  flowToConfig,
  recomputeNodeData,
} from "./components/flow-utils";
import { FlowCanvas } from "./components/FlowCanvas";
import { SidePanel } from "./components/SidePanel";
import type { Selection } from "./components/SidePanel";
import { TomlPreview } from "./components/TomlPreview";
import { ValidationPanel } from "./components/ValidationPanel";
import { api, type AudioDevice, type ConfigStatusResponse } from "./lib/api";
import { cascadeHidden, disconnectedDeviceNames } from "./lib/graph";
import type { AudiorouterConfig, DeviceConfig, RouteConfig } from "./types";
import { createEmptyConfig } from "./types";

type LoadState = "loading" | "loaded" | "error";
type BottomTab = "validation" | "toml";

function configFingerprint(config: AudiorouterConfig): string {
  return JSON.stringify(config);
}

function emptyConfigStatus(): ConfigStatusResponse {
  return {
    errors: [],
    warnings: [],
    unavailableInputs: [],
    unavailableOutputs: [],
    disabledRouteIndices: [],
    missingDeviceAliases: [],
  };
}

export default function App() {
  const [config, setConfig] = useState<AudiorouterConfig>(createEmptyConfig());
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState("");
  const [configPath, setConfigPath] = useState("");
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [savedConfigFingerprint, setSavedConfigFingerprint] = useState<string | null>(null);
  const [configStatus, setConfigStatus] = useState<ConfigStatusResponse>(() => emptyConfigStatus());
  const [activeBottomTab, setActiveBottomTab] = useState<BottomTab | null>(null);
  const [tomlPreview, setTomlPreview] = useState("");
  const [selection, setSelection] = useState<Selection>({ kind: "none" });
  const [isInteractive, setIsInteractive] = useState(true);

  // ── Device visibility toggles (mirror tui.rs 'h'/'H' keys) ──
  // showDisconnected: show devices not participating in any route
  // showMissing:  show devices whose hardware is not found on the system
  const [showDisconnected, setShowDisconnected] = useState(false);
  // Mirrors tui.rs default: missing devices are shown (dimmed) unless hidden with H.
  const [showMissing, setShowMissing] = useState(true);

  // ── Known CoreAudio devices (for autocomplete + missing detection) ──
  const [availableDevices, setAvailableDevices] = useState<AudioDevice[]>([]);

  // React Flow state
  const initial = useMemo(() => configToFlow(config), []);
  const [nodes, setNodes, onNodesChange] = useNodesState(initial.nodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initial.edges);

  // ── Load config + devices on mount ──────────────────────
  useEffect(() => {
    api
      .loadConfig()
      .then((res) => {
        setConfig(res.config);
        setConfigPath(res.path);
        const flow = configToFlow(res.config);
        setNodes(flow.nodes);
        setEdges(flow.edges);
        setSavedConfigFingerprint(configFingerprint(flowToConfig(flow, res.config.engine)));
        setSaveState("idle");
        setLoadState("loaded");
      })
      .catch((e) => {
        setLoadError(e instanceof Error ? e.message : String(e));
        setLoadState("error");
      });

    // Fetch available CoreAudio devices (non-blocking — empty on failure)
    api
      .listDevices()
      .then((res) => setAvailableDevices(res.all))
      .catch(() => {
        /* silently ignore — device list is best-effort */
      });
  }, []);

  // ── SSE: listen for device changes pushed from audiorouter-dashboard-api ──
  // When CoreAudio devices connect/disconnect, the backend emits
  // `devices_changed` events. We refresh the device list and config status
  // so the UI reflects the new connectivity without manual refresh.
  const [sseDeviceVersion, setSseDeviceVersion] = useState(0);
  const [configFileChanged, setConfigFileChanged] = useState(false);

  const sseRef = useRef<EventSource | null>(null);

  useEffect(() => {
    const es = new EventSource("/api/events");
    sseRef.current = es;

    es.addEventListener("devices_changed", () => {
      // Refresh device inventory
      api
        .listDevices()
        .then((res) => setAvailableDevices(res.all))
        .catch(() => {});
      // Trigger status re-evaluation by bumping the fingerprint dependency
      setSseDeviceVersion((v) => v + 1);
    });

    es.addEventListener("config_changed", () => {
      setConfigFileChanged(true);
    });

    es.onerror = () => {
      // EventSource auto-reconnects; nothing to do here
    };

    return () => {
      es.close();
      sseRef.current = null;
    };
  }, []);

  // ── Derive config from flow state ──────────────────────
  const currentConfig = useMemo(
    () => flowToConfig({ nodes, edges }, config.engine),
    [nodes, edges, config.engine],
  );
  const currentConfigFingerprint = useMemo(() => configFingerprint(currentConfig), [currentConfig]);
  const isDirty =
    savedConfigFingerprint !== null && currentConfigFingerprint !== savedConfigFingerprint;

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.previewConfig(currentConfig), api.statusConfig(currentConfig)])
      .then(([preview, status]) => {
        if (cancelled) return;
        setTomlPreview(preview.raw);
        setConfigStatus(status);
      })
      .catch(() => {
        if (cancelled) return;
        setTomlPreview("# TOML preview unavailable");
        setConfigStatus(emptyConfigStatus());
      });
    return () => {
      cancelled = true;
    };
  }, [currentConfig, sseDeviceVersion]);

  const allErrors = configStatus.errors;
  const clientWarnings = configStatus.warnings;

  // ── Compute device visibility (mirrors tui.rs) ──────────
  // Use effective names throughout (name="" → device)
  const deviceNames = currentConfig.devices.map((d) => d.name || d.device);
  const routeEdges = currentConfig.routes.map((r) => ({
    from: r.from,
    to: r.to,
  }));

  // Device inventory is still kept client-side for autocomplete and channel badges.
  const availableByName = useMemo(
    () => new Map(availableDevices.map((d) => [d.name, d])),
    [availableDevices],
  );

  // Availability and disabled-route semantics come from audiorouter-core via
  // /api/config/status. Keep React-specific visibility/cascade logic local.
  const unavailableInputs = useMemo(
    () => new Set(configStatus.unavailableInputs),
    [configStatus.unavailableInputs],
  );
  const unavailableOutputs = useMemo(
    () => new Set(configStatus.unavailableOutputs),
    [configStatus.unavailableOutputs],
  );
  const missingSet = useMemo(
    () => new Set(configStatus.missingDeviceAliases),
    [configStatus.missingDeviceAliases],
  );
  const disabledRouteIndices = useMemo(
    () => new Set(configStatus.disabledRouteIndices),
    [configStatus.disabledRouteIndices],
  );

  // Disconnected devices: not participating in any route
  const disconnectedSet = useMemo(
    () => new Set(disconnectedDeviceNames(deviceNames, routeEdges)),
    [deviceNames, routeEdges],
  );

  // Build the initial hidden set based on toggle states
  const hiddenSet = useMemo(() => {
    const hidden = new Set<string>();
    if (!showDisconnected) {
      for (const name of disconnectedSet) hidden.add(name);
    }
    if (!showMissing) {
      for (const name of missingSet) hidden.add(name);
    }
    // Cascade: after hiding, devices that lose all routes also get hidden
    return cascadeHidden(deviceNames, routeEdges, hidden);
  }, [disconnectedSet, missingSet, showDisconnected, showMissing, deviceNames, routeEdges]);

  // Counters for toggle button badges
  const disconnectedCount = disconnectedSet.size;
  const missingCount = missingSet.size;

  // Apply resolved availability to graph data for rendering.
  // This mirrors tui.rs:
  // - draw_device_node dims unavailable aliases
  // - draw_edge dims/OFFs routes where route_enabled(index) is false
  const resolvedNodes = useMemo(
    () =>
      nodes.map((node) => {
        const data = node.data as DeviceNodeData;
        const alias = data.name || data.device;
        const hardware = availableByName.get(data.device);
        const totalIn = hardware?.maxInputChannels ?? data.channels.totalIn;
        const totalOut = hardware?.maxOutputChannels ?? data.channels.totalOut;
        const hardwareMissing = missingSet.has(alias);
        return {
          ...node,
          data: {
            ...data,
            channels: {
              ...data.channels,
              totalIn,
              totalOut,
            },
            missingInput: unavailableInputs.has(alias) || hardwareMissing,
            missingOutput: unavailableOutputs.has(alias) || hardwareMissing,
          },
        };
      }),
    [nodes, availableByName, missingSet, unavailableInputs, unavailableOutputs],
  );

  const resolvedEdges = useMemo(
    () =>
      edges.map((edge, index) => {
        const data = edge.data as RouteEdgeData | undefined;
        if (!data) return edge;
        const disabled = disabledRouteIndices.has(index);
        return {
          ...edge,
          animated: !data.mute && !disabled,
          data: {
            ...data,
            disabled,
          },
        };
      }),
    [edges, disabledRouteIndices],
  );

  // ── Filtered flow state for rendering ───────────────────
  const { filteredNodes, filteredEdges } = useMemo(() => {
    const hiddenIds = new Set([...hiddenSet].map((name) => `device-${name}`));

    // Selected items are always shown regardless of filters.
    const pinnedNodeIds = new Set<string>();
    if (selection.kind === "device") {
      pinnedNodeIds.add(selection.id);
    } else if (selection.kind === "edge") {
      const selectedEdge = resolvedEdges.find((e) => e.id === selection.id);
      if (selectedEdge) {
        pinnedNodeIds.add(selectedEdge.source);
        pinnedNodeIds.add(selectedEdge.target);
      }
    }

    const isHiddenNode = (n: (typeof resolvedNodes)[number]) => {
      if (pinnedNodeIds.has(n.id)) return false;
      if (hiddenIds.has(n.id)) return true;
      const d = n.data as DeviceNodeData;
      const alias = d.name || d.device;
      return !!alias && hiddenIds.has(`device-${alias}`);
    };

    const filteredNodes = resolvedNodes.filter((n) => !isHiddenNode(n));
    const visibleNodeIds = new Set(filteredNodes.map((n) => n.id));

    const filteredEdges = resolvedEdges
      .filter((e) => {
        if (selection.kind === "edge" && e.id === selection.id) return true;
        return visibleNodeIds.has(e.source) && visibleNodeIds.has(e.target);
      })
      .sort((a, b) => {
        // Selected edges render last (on top). Among non-selected, dim edges
        // render before non-dim so non-dim paths appear above dim paths.
        if (a.selected !== b.selected) return a.selected ? 1 : -1;
        const ad = a.data as RouteEdgeData | undefined;
        const bd = b.data as RouteEdgeData | undefined;
        const aDim = ad?.disabled || ad?.mute ? 0 : 1;
        const bDim = bd?.disabled || bd?.mute ? 0 : 1;
        return aDim - bDim;
      });
    return { filteredNodes, filteredEdges };
  }, [resolvedNodes, resolvedEdges, hiddenSet, selection]);

  // ── Edit guard ──────────────────────────────────────────
  // Single source of truth for the lock check.
  // Every callback that mutates config must call this first.
  // Acts as a safety net even when a UI component forgets disabled={!isInteractive}.
  const canEdit = useCallback((): boolean => isInteractive, [isInteractive]);

  // ── Flow callbacks ──────────────────────────────────────

  // Filter out destructive/positional changes when the canvas is locked.
  // "remove" changes are triggered by the Delete key; "position" by dragging.
  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      if (!canEdit()) {
        const safe = changes.filter((c) => c.type !== "remove" && c.type !== "position");
        if (safe.length > 0) onNodesChange(safe);
        return;
      }
      onNodesChange(changes);
    },
    [canEdit, onNodesChange],
  );

  const handleEdgesChange = useCallback(
    (changes: EdgeChange[]) => {
      if (!canEdit()) {
        const safe = changes.filter((c) => c.type !== "remove");
        if (safe.length > 0) onEdgesChange(safe);
        return;
      }
      onEdgesChange(changes);
    },
    [canEdit, onEdgesChange],
  );

  const handleConnect = useCallback(
    (conn: Connection) => {
      if (!canEdit()) return;
      const sourceNode = nodes.find((n) => n.id === conn.source);
      const targetNode = nodes.find((n) => n.id === conn.target);
      if (!sourceNode || !targetNode) return;

      const newEdge = createRouteEdge(sourceNode, targetNode, edges.length);
      setEdges((eds) => addEdgeSafe(eds, newEdge));

      const cfg = flowToConfig({ nodes, edges: [...edges, newEdge] }, config.engine);
      setNodes((nds) => recomputeNodeData(nds, cfg));
    },
    [canEdit, nodes, edges, config.engine, setEdges, setNodes],
  );

  const handleNodeClick = useCallback((nodeId: string) => {
    setSelection({ kind: "device", id: nodeId });
  }, []);

  const handleEdgeClick = useCallback((edgeId: string) => {
    setSelection({ kind: "edge", id: edgeId });
  }, []);

  const handlePaneClick = useCallback(() => {
    setSelection({ kind: "none" });
  }, []);

  const [layoutVersion, setLayoutVersion] = useState(0);

  const handleLayout = useCallback(() => {
    setNodes((nds) => applyAutoLayout(nds, edges));
    setLayoutVersion((v) => v + 1);
  }, [edges, setNodes]);

  const handleToggleInteractive = useCallback(() => {
    setIsInteractive((v) => !v);
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelection({ kind: "none" });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // Keep node/edge .selected in sync with our selection state.
  // Needed when the canvas is locked: React Flow won't update .selected
  // on click because elementsSelectable is false in its store.
  useEffect(() => {
    setNodes((nds) =>
      nds.map((n) => ({
        ...n,
        selected: selection.kind === "device" && n.id === selection.id,
      })),
    );
    setEdges((eds) =>
      eds.map((e) => ({
        ...e,
        selected: selection.kind === "edge" && e.id === selection.id,
      })),
    );
  }, [selection, setNodes, setEdges]);

  // ── Device operations ───────────────────────────────────
  const handleAddDevice = useCallback(() => {
    if (!canEdit()) return;
    // Use a UUID-based placeholder so the ID follows the device-* convention
    // from the start. handleUpdateDevice renames it to device-${alias} once the
    // user sets a name or device field.
    const id = `device-${crypto.randomUUID().slice(0, 8)}`;
    const data: DeviceNodeData = {
      name: "", // empty — uses device name as alias at runtime
      device: "",
      limiter: false,
      role: "input",
      channels: { chIn: 0, chOut: 0, totalIn: 0, totalOut: 0 },
      missingInput: false,
      missingOutput: false,
    };
    const newNode: Node = {
      id,
      type: "device",
      position: { x: 300 + Math.random() * 100, y: 100 + Math.random() * 100 },
      data,
    };
    setNodes((nds) => [...nds, newNode]);
    setSelection({ kind: "device", id });
  }, [canEdit, setNodes]);

  const handleUpdateDevice = useCallback(
    (id: string, patch: Partial<DeviceConfig>) => {
      if (!canEdit()) return;

      const targetNode = nodes.find((n) => n.id === id);
      if (!targetNode) return;
      const oldData = targetNode.data as DeviceNodeData;
      const oldAlias = oldData.name || oldData.device;
      const newName = "name" in patch ? (patch.name ?? "") : oldData.name;
      const newDevice = "device" in patch ? (patch.device ?? "") : oldData.device;
      const newAlias = newName || newDevice;

      // Keep node ID aligned with alias: device-${alias}.
      // Fall back to the current ID when alias is still empty (unnamed device).
      const newId = newAlias ? `device-${newAlias}` : id;

      setNodes((nds) =>
        nds.map((n) => {
          if (n.id !== id) return n;
          return { ...n, id: newId, data: { ...oldData, ...patch } };
        }),
      );

      // When alias or node ID changes, update all edges that reference it.
      if (oldAlias !== newAlias || newId !== id) {
        setEdges((eds) =>
          eds.map((e) => {
            const data = e.data as RouteEdgeData | undefined;
            const newSource = e.source === id ? newId : e.source;
            const newTarget = e.target === id ? newId : e.target;
            const aliasChanged = data && oldAlias !== newAlias;
            return {
              ...e,
              source: newSource,
              target: newTarget,
              data: aliasChanged
                ? {
                    ...data,
                    from: data.from === oldAlias ? newAlias : data.from,
                    to: data.to === oldAlias ? newAlias : data.to,
                  }
                : e.data,
            };
          }),
        );
      }

      // Keep selection in sync when the node ID changes.
      if (newId !== id) {
        setSelection((sel) =>
          sel.kind === "device" && sel.id === id ? { kind: "device", id: newId } : sel,
        );
      }
    },
    [canEdit, nodes, setNodes, setEdges, setSelection],
  );

  const handleDeleteDevice = useCallback(
    (id: string) => {
      if (!canEdit()) return;
      setNodes((nds) => nds.filter((n) => n.id !== id));
      setEdges((eds) => eds.filter((e) => e.source !== id && e.target !== id));
      setSelection({ kind: "none" });
    },
    [canEdit, setNodes, setEdges],
  );

  // ── Route operations ────────────────────────────────────
  const handleUpdateRoute = useCallback(
    (id: string, patch: Partial<RouteConfig>) => {
      if (!canEdit()) return;
      setEdges((eds) =>
        eds.map((e) => {
          if (e.id !== id) return e;
          const oldData = e.data as RouteEdgeData;
          const newData: RouteEdgeData = { ...oldData, ...patch };
          return {
            ...e,
            data: newData,
            animated: !newData.mute,
          };
        }),
      );
    },
    [canEdit, setEdges],
  );

  const handleDeleteRoute = useCallback(
    (id: string) => {
      if (!canEdit()) return;
      setEdges((eds) => eds.filter((e) => e.id !== id));
      setSelection({ kind: "none" });
    },
    [canEdit, setEdges],
  );

  // ── Engine ──────────────────────────────────────────────
  const handleEngineChange = useCallback(
    (engine: AudiorouterConfig["engine"]) => {
      if (!canEdit()) return;
      setConfig((c) => ({ ...c, engine }));
    },
    [canEdit],
  );

  // ── Save / Reload ───────────────────────────────────────
  const handleSave = useCallback(async () => {
    if (!isDirty) return;
    setSaveState("saving");
    const cfg = flowToConfig({ nodes, edges }, config.engine);
    try {
      const res = await api.saveConfig(cfg);
      if (res.errors.length > 0) {
        setConfigStatus((status) => ({ ...status, errors: res.errors }));
        setSaveState("error");
      } else {
        setConfigStatus(await api.statusConfig(cfg));
        setSavedConfigFingerprint(configFingerprint(cfg));
        setSaveState("saved");
        setTimeout(() => setSaveState("idle"), 2000);
      }
    } catch {
      setSaveState("error");
    }
  }, [isDirty, nodes, edges, config.engine]);

  const handleReload = useCallback(async () => {
    if (isDirty) {
      const ok = window.confirm(
        "未保存の変更があります。破棄して設定ファイルを再読み込みしますか？",
      );
      if (!ok) return;
    }
    setLoadState("loading");
    try {
      const res = await api.loadConfig();
      setConfig(res.config);
      setConfigPath(res.path);
      const flow = configToFlow(res.config);
      setNodes(flow.nodes);
      setEdges(flow.edges);
      setSavedConfigFingerprint(configFingerprint(flowToConfig(flow, res.config.engine)));
      setSaveState("idle");
      setConfigStatus(emptyConfigStatus());
      setSelection({ kind: "none" });
      setConfigFileChanged(false);
      setLoadState("loaded");
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
      setLoadState("error");
    }
  }, [isDirty, setNodes, setEdges]);

  // ── Loading states ──────────────────────────────────────
  if (loadState === "loading") {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="animate-pulse text-sm text-[var(--color-muted-foreground)]">
          設定ファイルを読み込んでいます…
        </div>
      </div>
    );
  }

  if (loadState === "error") {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="text-center">
          <p className="mb-2 font-mono text-sm" style={{ color: "var(--color-destructive)" }}>
            読み込みエラー
          </p>
          <p className="mb-4 font-mono text-xs text-[var(--color-muted-foreground)]">{loadError}</p>
          <button
            type="button"
            onClick={handleReload}
            className="rounded-md bg-[var(--color-primary)] px-4 py-2 text-sm text-[var(--color-primary-foreground)] transition hover:opacity-90"
          >
            再読み込み
          </button>
        </div>
      </div>
    );
  }

  const isValid = allErrors.length === 0;
  const saveDisabled = !isValid || saveState === "saving" || !isDirty;
  const toggleBottomTab = (tab: BottomTab) => {
    setActiveBottomTab((current) => (current === tab ? null : tab));
  };

  return (
    <div className="flex h-full flex-col">
      {/* ── Top bar ────────────────────────────────────── */}
      <header className="flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-card)] px-4 py-2.5">
        <div className="flex items-center gap-4">
          <h1 className="text-sm font-bold tracking-tight text-[var(--color-foreground)]">
            <span style={{ color: "var(--color-ar-border)" }}>audio</span>router
            <span className="ml-1.5 text-xs font-normal text-[var(--color-muted-foreground)]">
              dashboard
            </span>
          </h1>
          <EngineBar
            engine={config.engine}
            onChange={handleEngineChange}
            readOnly={!isInteractive}
          />
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={handleReload}
            className={`flex h-7 w-24 items-center justify-center gap-1.5 rounded-md border text-xs transition ${
              configFileChanged
                ? "animate-pulse border-[var(--color-ar-border)] bg-[color-mix(in_oklch,var(--color-ar-border)_15%,transparent)] font-semibold text-[var(--color-ar-border)] hover:bg-[color-mix(in_oklch,var(--color-ar-border)_25%,transparent)]"
                : "border-[var(--color-border)] text-[var(--color-muted-foreground)] hover:bg-[var(--color-muted)]"
            }`}
            title={
              configFileChanged
                ? "設定ファイルが外部で変更されました — クリックして再読み込み"
                : "設定ファイルから再読み込み"
            }
          >
            <svg
              width="11"
              height="11"
              viewBox="0 0 11 11"
              fill="none"
              className={configFileChanged ? "animate-spin" : ""}
            >
              <path
                d="M9.5 5.5A4 4 0 1 1 8 2.2"
                stroke="currentColor"
                strokeWidth="1.4"
                strokeLinecap="round"
              />
              <polyline
                points="7,0.5 9.5,2 7.5,4"
                stroke="currentColor"
                strokeWidth="1.4"
                strokeLinecap="round"
                strokeLinejoin="round"
                fill="none"
              />
            </svg>
            <span>再読込</span>
          </button>

          <button
            type="button"
            disabled={saveDisabled}
            onClick={handleSave}
            title={
              !isDirty
                ? "保存済みです"
                : !isValid
                  ? "エラーがあるため保存できません"
                  : "設定ファイルへ保存"
            }
            className="flex h-7 w-24 items-center justify-center gap-1.5 rounded-md text-xs font-medium transition disabled:cursor-not-allowed"
            style={
              saveState === "error"
                ? {
                    background: "color-mix(in oklch, var(--color-destructive) 15%, transparent)",
                    color: "var(--color-destructive)",
                    border: "1px solid var(--color-destructive)",
                  }
                : saveDisabled
                  ? {
                      background: "var(--color-muted)",
                      color: "var(--color-muted-foreground)",
                      opacity: 0.5,
                    }
                  : {
                      background: "var(--color-primary)",
                      color: "var(--color-primary-foreground)",
                    }
            }
          >
            {saveState === "saving" ? (
              <svg className="animate-spin" width="11" height="11" viewBox="0 0 11 11" fill="none">
                <circle
                  cx="5.5"
                  cy="5.5"
                  r="4"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeDasharray="12 14"
                  strokeLinecap="round"
                />
              </svg>
            ) : saveState === "error" ? (
              <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
                <line x1="2" y1="2" x2="9" y2="9" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
                <line x1="9" y1="2" x2="2" y2="9" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
              </svg>
            ) : !isDirty ? (
              <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
                <polyline
                  points="1.5,5.5 4,8 9.5,2.5"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            ) : (
              <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
                <path
                  d="M5.5 1.5v6M2.5 5l3 3 3-3"
                  stroke="currentColor"
                  strokeWidth="1.4"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
                <line x1="1.5" y1="9.5" x2="9.5" y2="9.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
              </svg>
            )}
            <span>
              {saveState === "saving"
                ? "保存中"
                : saveState === "error"
                  ? "エラー"
                  : !isDirty
                    ? "保存済"
                    : "保存"}
            </span>
          </button>
        </div>
      </header>

      {/* ── Main area: canvas + side panel ───────────── */}
      <div className="flex flex-1 overflow-hidden">
        {/* Canvas */}
        <div className="relative flex-1">
          <FlowCanvas
            nodes={filteredNodes}
            edges={filteredEdges}
            onNodesChange={handleNodesChange}
            onEdgesChange={handleEdgesChange}
            onConnect={handleConnect}
            onNodeClick={handleNodeClick}
            onEdgeClick={handleEdgeClick}
            onPaneClick={handlePaneClick}
            onLayout={handleLayout}
            layoutVersion={layoutVersion}
            isInteractive={isInteractive}
            onToggleInteractive={handleToggleInteractive}
            onUpdateRoute={handleUpdateRoute}
          />
          {/* Floating "Add Device" button */}
          <button
            type="button"
            onClick={handleAddDevice}
            disabled={!isInteractive}
            className="absolute top-3 left-3 z-10 rounded-md border border-[var(--color-border)] bg-[var(--color-card)] px-3 py-1.5 text-xs text-[var(--color-foreground)] shadow-md transition hover:bg-[var(--color-muted)] disabled:opacity-40 disabled:cursor-not-allowed"
          >
            + デバイス追加
          </button>
          {/* Visibility filter checkboxes */}
          <div className="absolute top-3 right-3 z-10 flex flex-col gap-1.5 rounded-md border border-[var(--color-border)] bg-[var(--color-card)]/90 px-3 py-2 shadow-md backdrop-blur">
            <FilterCheckbox
              checked={showDisconnected}
              onChange={() => setShowDisconnected((v) => !v)}
              label="Disconnected"
              count={disconnectedCount}
            />
            <FilterCheckbox
              checked={showMissing}
              onChange={() => setShowMissing((v) => !v)}
              label="Missing"
              count={missingCount}
            />
          </div>
          {/* Config path badge */}
          {configPath && (
            <div className="absolute right-3 bottom-3 z-10 rounded bg-[var(--color-card)]/80 px-2 py-1 font-mono text-[10px] text-[var(--color-muted-foreground)] backdrop-blur">
              {configPath}
            </div>
          )}
        </div>

        {/* Side panel */}
        <aside className="w-72 shrink-0 overflow-y-auto border-l border-[var(--color-border)] bg-[var(--color-card)]">
          <SidePanel
            selection={selection}
            nodes={resolvedNodes}
            edges={resolvedEdges}
            config={currentConfig}
            onUpdateDevice={handleUpdateDevice}
            onUpdateRoute={handleUpdateRoute}
            onDeleteDevice={handleDeleteDevice}
            onDeleteRoute={handleDeleteRoute}
            onAddDevice={handleAddDevice}
            availableDevices={availableDevices}
            readOnly={!isInteractive}
          />
        </aside>
      </div>

      {/* ── Bottom panel: IDE-like tabs for validation + TOML ─────────── */}
      <section
        className={`shrink-0 border-t border-[var(--color-border)] bg-[var(--color-card)] ${
          activeBottomTab ? "h-64" : "h-9"
        }`}
      >
        <div className="flex h-full flex-col">
          <div
            className={`flex h-9 shrink-0 items-end bg-[var(--color-muted)]/35 px-3 ${
              activeBottomTab ? "border-b border-[var(--color-border)]" : ""
            }`}
          >
            <BottomTabButton
              active={activeBottomTab === "validation"}
              statusLabel={isValid ? "valid" : `${allErrors.length} err`}
              statusTone={isValid ? "ok" : "error"}
              badge={allErrors.length > 0 ? allErrors.length : clientWarnings.length}
              tone={allErrors.length > 0 ? "error" : clientWarnings.length > 0 ? "warning" : "ok"}
              onClick={() => toggleBottomTab("validation")}
            />
            <BottomTabButton
              active={activeBottomTab === "toml"}
              label="config.toml"
              onClick={() => toggleBottomTab("toml")}
            />
          </div>
          {activeBottomTab && (
            <div className="min-h-0 flex-1 overflow-y-auto p-3">
              {activeBottomTab === "validation" ? (
                <ValidationPanel errors={allErrors} warnings={clientWarnings} />
              ) : (
                <TomlPreview toml={tomlPreview} />
              )}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

/**
 * IDE-like bottom panel tab.
 */
function BottomTabButton({
  active,
  label,
  statusLabel,
  statusTone,
  badge,
  tone,
  onClick,
}: {
  active: boolean;
  label?: string;
  statusLabel?: string;
  statusTone?: "ok" | "error";
  badge?: number;
  tone?: "ok" | "warning" | "error";
  onClick: () => void;
}) {
  const statusColor = statusTone === "error" ? "var(--color-destructive)" : "var(--color-ar-in)";
  const badgeColor =
    tone === "error"
      ? "var(--color-destructive)"
      : tone === "warning"
        ? "var(--color-ar-gain)"
        : "var(--color-ar-in)";

  return (
    <button
      type="button"
      onClick={onClick}
      className="relative -mb-px flex h-9 items-center gap-2 border-x border-t px-3 text-xs font-medium transition"
      style={
        active
          ? {
              borderColor: "var(--color-border)",
              borderBottomColor: "var(--color-card)",
              background: "var(--color-card)",
              color: "var(--color-foreground)",
            }
          : {
              borderColor: "transparent",
              background: "transparent",
              color: "var(--color-muted-foreground)",
            }
      }
    >
      {label && <span>{label}</span>}
      {statusLabel && (
        <span
          className="inline-flex items-center gap-1 font-mono text-[10px]"
          style={{ color: statusColor }}
        >
          <span className="h-1.5 w-1.5 rounded-full" style={{ background: statusColor }} />
          {statusLabel}
        </span>
      )}
      {badge !== undefined && badge > 0 && (
        <span
          className="rounded-full px-1.5 py-0.5 font-mono text-[10px] leading-none"
          style={{
            background: `color-mix(in oklch, ${badgeColor} 18%, transparent)`,
            color: badgeColor,
          }}
        >
          {badge}
        </span>
      )}
    </button>
  );
}

/**
 * Toggle button for device visibility (mirrors tui.rs 'h'/'H' keys).
 * Shows a badge with the count of hidden devices.
 */
function FilterCheckbox({
  checked,
  onChange,
  label,
  count,
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
  count: number;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-2 select-none">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        className="h-3.5 w-3.5 cursor-pointer rounded"
      />
      <span className="text-xs font-medium" style={{ color: "var(--color-foreground)" }}>
        {label}
      </span>
      <span
        className="rounded-full px-1.5 py-0.5 text-[10px] font-bold leading-none"
        style={{
          background: count > 0 ? "var(--color-secondary)" : "var(--color-muted)",
          color: count > 0 ? "var(--color-secondary-foreground)" : "var(--color-muted-foreground)",
        }}
      >
        {count}
      </span>
    </label>
  );
}

function addEdgeSafe(edges: Edge[], newEdge: Edge): Edge[] {
  const exists = edges.some((e) => e.source === newEdge.source && e.target === newEdge.target);
  if (exists) return edges;
  return [...edges, newEdge];
}
