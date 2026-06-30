import { createContext, useContext } from "react";
import type { RouteConfig } from "../types";

/**
 * Provides a callback to update a route's data from within a custom edge
 * component. React Flow does not allow passing arbitrary props to edge
 * components, so we use Context to bridge App → FlowCanvas → RouteEdge.
 */
export type RouteUpdateFn = (id: string, patch: Partial<RouteConfig>) => void;

const RouteUpdateContext = createContext<RouteUpdateFn | null>(null);

export const RouteUpdateProvider = RouteUpdateContext.Provider;

export function useRouteUpdate(): RouteUpdateFn | null {
  return useContext(RouteUpdateContext);
}
