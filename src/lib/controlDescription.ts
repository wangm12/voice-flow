import { createContext, useContext } from "react";

export const ControlDescriptionContext = createContext<string | undefined>(undefined);

export function useControlDescription(explicit?: string): string | undefined {
  const contextual = useContext(ControlDescriptionContext);
  return [...new Set([explicit, contextual].filter(Boolean))].join(" ") || undefined;
}
