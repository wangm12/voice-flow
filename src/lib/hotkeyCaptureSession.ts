import { invoke } from "@tauri-apps/api/core";

export type CaptureTarget = "dictation" | "selected_action" | "screen_action" | "verbatim_action" | "translation_action";
type Session = ReturnType<typeof makeSession>;
let queue: Promise<void> = Promise.resolve();
let current: Session | null = null;

function serial<T>(operation: () => Promise<T>): Promise<T> {
  const result = queue.then(operation);
  queue = result.then(() => undefined, () => undefined);
  return result;
}

function makeSession(target: CaptureTarget) {
  let cancelled = false;
  let completion: Promise<string | null> | null = null;
  const ready = serial(async () => {
    if (cancelled) return false;
    await invoke("set_hotkeys_suspended", { suspended: true, capturedHotkey: null, captureTarget: target });
    return true;
  });
  const session = {
    ready,
    get finishing() { return completion !== null; },
    finish(hotkey: string | null): Promise<string | null> {
      if (completion) return completion;
      if (hotkey === null) cancelled = true;
      completion = serial(async () => {
        if (!await ready) return null;
        await invoke("set_hotkeys_suspended", { suspended: false, capturedHotkey: hotkey, captureTarget: target });
        return hotkey;
      }).finally(() => { if (current === session) current = null; });
      return completion;
    },
  };
  void ready.catch(() => { if (current === session) current = null; });
  return session;
}

/** Serialize native capture across pages, including a departed recorder's pending release. */
export function beginHotkeyCapture(target: CaptureTarget) {
  if (current && !current.finishing) throw new Error("已有快捷键正在编辑，请先取消。");
  current = makeSession(target);
  return current;
}
