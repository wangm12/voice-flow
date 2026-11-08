import { createContext, useCallback, useContext, useEffect, useRef, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";

type DraftStore = { values: Map<string, unknown>; owners: Map<string, symbol> };
const SessionDraftContext = createContext<DraftStore | null>(null);

/** Drafts live only for this settings window. No storage, IPC, or hidden mounted pages. */
export function SessionDraftProvider({ children }: { children: ReactNode }) {
  const drafts = useRef<DraftStore>({ values: new Map(), owners: new Map() });
  return <SessionDraftContext.Provider value={drafts.current}>{children}</SessionDraftContext.Provider>;
}

export function useSessionDraftState<T>(key: string, initial: T | (() => T)): [T, Dispatch<SetStateAction<T>>, boolean] {
  const drafts = useContext(SessionDraftContext);
  const owner = useRef(Symbol(key));
  const restored = useRef(drafts?.values.has(key) ?? false);
  const [value, setValue] = useState<T>(() => drafts?.values.has(key)
    ? drafts.values.get(key) as T
    : typeof initial === "function" ? (initial as () => T)() : initial);
  const latest = useRef(value);
  latest.current = value;
  useEffect(() => { drafts?.owners.set(key, owner.current); }, [drafts, key]);
  useEffect(() => {
    if (drafts?.owners.get(key) === owner.current) drafts.values.set(key, value);
  }, [drafts, key, value]);
  useEffect(() => () => {
    if (drafts?.owners.get(key) === owner.current) drafts.values.set(key, latest.current);
  }, [drafts, key]);
  const updateValue = useCallback<Dispatch<SetStateAction<T>>>((action) => {
    if (drafts && drafts.owners.get(key) !== owner.current) return;
    const next = typeof action === "function" ? (action as (previous: T) => T)(latest.current) : action;
    latest.current = next;
    if (drafts?.owners.get(key) === owner.current) drafts.values.set(key, next);
    setValue(next);
  }, [drafts, key]);
  return [value, updateValue, restored.current];
}
