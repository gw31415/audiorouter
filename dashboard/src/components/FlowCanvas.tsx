import {
  ReactFlowProvider,
  type OnConnect,
  useReactFlow,
  useNodesInitialized,
} from "@xyflow/react";
import {
  Background,
  BackgroundVariant,
  ControlButton,
  Controls,
  ReactFlow,
  type Edge,
  type Node,
  type OnEdgesChange,
  type OnNodesChange,
} from "@xyflow/react";
import { useEffect } from "react";
import "@xyflow/react/dist/style.css";
import { DeviceNode } from "./DeviceNode";
import { RouteUpdateProvider, type RouteUpdateFn } from "./route-update-context";
import { RouteEdge } from "./RouteEdge";

const nodeTypes = { device: DeviceNode };
const edgeTypes = { route: RouteEdge };
const proOptions = { hideAttribution: true };

interface Props {
  nodes: Node[];
  edges: Edge[];
  onNodesChange: OnNodesChange;
  onEdgesChange: OnEdgesChange;
  onConnect: OnConnect;
  onNodeClick: (nodeId: string) => void;
  onEdgeClick: (edgeId: string) => void;
  onPaneClick: () => void;
  onLayout: () => void;
  layoutVersion: number;
  isInteractive: boolean;
  onToggleInteractive: () => void;
  onUpdateRoute: RouteUpdateFn;
}

function FlowCanvasInner({
  nodes,
  edges,
  onNodesChange,
  onEdgesChange,
  onConnect,
  onNodeClick,
  onEdgeClick,
  onPaneClick,
  onLayout,
  layoutVersion,
  isInteractive,
  onToggleInteractive,
  onUpdateRoute,
}: Props) {
  const { fitView } = useReactFlow();
  const nodesInitialized = useNodesInitialized();

  // Fire fitView after React has committed the new node positions.
  // useEffect runs post-commit; the extra rAF waits for React Flow's
  // own internal recalc before we ask for the bounding box.
  useEffect(() => {
    if (layoutVersion === 0) return;
    const id = requestAnimationFrame(() => {
      void fitView({ duration: 0, padding: 0.25 });
    });
    return () => cancelAnimationFrame(id);
  }, [layoutVersion, fitView]);

  // After initial node measurement, the invisible interaction paths for edges
  // are recalculated. Without this, edges can't be clicked until a node moves.
  useEffect(() => {
    if (!nodesInitialized) return;
    const id = requestAnimationFrame(() => {
      void fitView({ duration: 0, padding: 0.25 });
    });
    return () => cancelAnimationFrame(id);
  }, [nodesInitialized, fitView]);

  const handleLayout = () => {
    onLayout();
  };

  return (
    <RouteUpdateProvider value={onUpdateRoute}>
      <div data-locked={!isInteractive || undefined} className="h-full w-full">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={(_, node) => onNodeClick(node.id)}
          onEdgeClick={(_, edge) => onEdgeClick(edge.id)}
          onPaneClick={onPaneClick}
          selectNodesOnDrag={false}
          nodesDraggable={isInteractive}
          nodesConnectable={isInteractive}
          elementsSelectable={true}
          proOptions={proOptions}
          fitView
          fitViewOptions={{ padding: 0.25 }}
          defaultEdgeOptions={{ type: "route" }}
          colorMode="dark"
        >
          <Background
            variant={BackgroundVariant.Dots}
            gap={16}
            size={1}
            color="var(--color-border)"
          />
          <Controls
            showInteractive={false}
            fitViewOptions={{ padding: 0.25, duration: 0 }}
            className="!border-[var(--color-border)] !bg-[var(--color-card)]"
          >
            <ControlButton
              onClick={onToggleInteractive}
              title={isInteractive ? "キャンバスをロック" : "キャンバスのロックを解除"}
            >
              {isInteractive ? <UnlockedIcon /> : <LockedIcon />}
            </ControlButton>
            <ControlButton
              onClick={handleLayout}
              title="ノードを自動整列"
              disabled={!isInteractive}
            >
              <LayoutIcon />
            </ControlButton>
          </Controls>
        </ReactFlow>
      </div>
    </RouteUpdateProvider>
  );
}

export function FlowCanvas(props: Props) {
  return (
    <ReactFlowProvider>
      <FlowCanvasInner {...props} />
    </ReactFlowProvider>
  );
}

function LockedIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
      <path
        d="M5.5 7V5a2.5 2.5 0 015 0v2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <rect x="2.5" y="7" width="11" height="8" rx="1.5" />
    </svg>
  );
}

function UnlockedIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
      <path
        d="M5.5 7V5a2.5 2.5 0 015 0V2.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <rect x="2.5" y="7" width="11" height="8" rx="1.5" />
    </svg>
  );
}

function LayoutIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
      <rect x="1" y="5" width="5" height="6" rx="1.2" />
      <rect x="10" y="1" width="5" height="5" rx="1.2" />
      <rect x="10" y="10" width="5" height="5" rx="1.2" />
      <line x1="6" y1="8" x2="10" y2="3.5" stroke="currentColor" strokeWidth="1.3" />
      <line x1="6" y1="8" x2="10" y2="12.5" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}
