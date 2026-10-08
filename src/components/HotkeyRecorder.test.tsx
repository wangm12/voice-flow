import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { HotkeyRecorder } from "./HotkeyRecorder";
import { SettingsDisclosure } from "./SettingsLayout";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const optionalTargets = ["selected_action", "screen_action", "verbatim_action", "translation_action"] as const;
const input = () => screen.getByRole("textbox", { name: "录入新的快捷键" });
const trigger = () => screen.getByRole("button", { name: /^更改/ });
const deferred = () => {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
async function flush() { await act(async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); }); }
async function begin() {
  fireEvent.click(trigger());
  await waitFor(() => expect(input()).toHaveAttribute("aria-busy", "false"));
}
function pressCombination() {
  fireEvent.keyDown(input(), { key: "Meta", code: "MetaLeft", metaKey: true });
  fireEvent.keyDown(input(), { key: "Alt", code: "AltLeft", metaKey: true, altKey: true });
  fireEvent.keyDown(input(), { key: " ", code: "Space", metaKey: true, altKey: true });
}
function releaseCombination() {
  fireEvent.keyUp(window, { key: " ", code: "Space", metaKey: true, altKey: true });
  fireEvent.keyUp(window, { key: "Alt", code: "AltLeft", metaKey: true });
  fireEvent.keyUp(window, { key: "Meta", code: "MetaLeft" });
}

describe("HotkeyRecorder", () => {
  beforeEach(() => { invokeMock.mockResolvedValue(undefined); });
  afterEach(async () => { cleanup(); await flush(); invokeMock.mockReset(); vi.restoreAllMocks(); });

  it("opens an inline editor on semantic click, never on pointer down", async () => {
    render(<HotkeyRecorder value="Command+Alt+Space" compact onChange={vi.fn()} />);
    expect(trigger()).toHaveAccessibleName(/更改听写快捷键：.*(?:⌘|Ctrl).*Space/);
    fireEvent.pointerDown(trigger(), { button: 0 }); await flush();
    expect(invokeMock).not.toHaveBeenCalled();
    await begin();
    expect(input()).toHaveFocus();
    expect(screen.getByText(/^当前快捷键：.*Space$/)).toBeVisible();
    expect(invokeMock).toHaveBeenCalledWith("set_hotkeys_suspended", { suspended: true, capturedHotkey: null, captureTarget: "dictation" });
  });

  it("keeps its disclosure locked through preparation, capture and cancellation ACK", async () => {
    const pause = deferred(), restore = deferred();
    invokeMock.mockReturnValueOnce(pause.promise).mockReturnValueOnce(restore.promise);
    function Group() {
      const [busy, setBusy] = useState(false);
      return <SettingsDisclosure title="更多快捷键" locked={busy}>
        <HotkeyRecorder value="" captureTarget="selected_action" onChange={vi.fn()} onCaptureBusyChange={setBusy} />
      </SettingsDisclosure>;
    }
    render(<Group />);
    const disclosure = screen.getByRole("button", { name: "更多快捷键" });
    fireEvent.click(disclosure); fireEvent.click(trigger()); await flush();
    const expectLocked = () => {
      fireEvent.click(disclosure);
      expect(disclosure).toHaveAttribute("aria-disabled", "true");
      expect(disclosure).toHaveAttribute("aria-expanded", "true");
    };
    expect(screen.getByRole("status")).toHaveTextContent("准备录入…"); expectLocked();
    pause.resolve(); await flush(); expectLocked();
    fireEvent.keyDown(input(), { key: "Escape", code: "Escape" }); await flush();
    expect(screen.getByRole("status")).toHaveTextContent("正在取消…"); expectLocked();
    restore.resolve(); await flush();
    expect(disclosure).toHaveAttribute("aria-disabled", "false");
    fireEvent.click(disclosure); expect(disclosure).toHaveAttribute("aria-expanded", "false");
  });

  it("cancels during preparation and restores only after a late native pause ACK", async () => {
    const pause = deferred(); invokeMock.mockReturnValueOnce(pause.promise);
    const onChange = vi.fn();
    render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    fireEvent.click(trigger()); await flush();
    fireEvent.click(screen.getByRole("button", { name: "取消" })); await flush();
    expect(invokeMock).toHaveBeenCalledOnce();
    expect(screen.getByRole("status")).toHaveTextContent("正在取消…");
    pause.resolve(); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "dictation" });
    expect(onChange).not.toHaveBeenCalled();
    expect(trigger()).toHaveAccessibleName("更改听写快捷键：fn");
  });

  it("restores a departed recorder after its native pause acknowledges", async () => {
    const pause = deferred(); invokeMock.mockReturnValueOnce(pause.promise);
    const onChange = vi.fn(), onBusy = vi.fn();
    const view = render(<HotkeyRecorder value="" captureTarget="screen_action" onChange={onChange} onCaptureBusyChange={onBusy} />);
    fireEvent.click(trigger()); await flush(); view.unmount(); await flush();
    expect(invokeMock).toHaveBeenCalledOnce();
    pause.resolve(); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "screen_action" });
    expect(onBusy.mock.calls).toEqual([[true], [false]]);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not resume another recording when preparation fails after unmount", async () => {
    const pause = deferred(); invokeMock.mockReturnValueOnce(pause.promise);
    const onChange = vi.fn();
    const view = render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    fireEvent.click(trigger()); await flush(); view.unmount();
    pause.reject(new Error("录音或处理期间")); await flush();
    expect(invokeMock).toHaveBeenCalledOnce(); expect(onChange).not.toHaveBeenCalled();
  });

  it.each([false, true])("Esc cancels even with held modifiers: %s", async (metaKey) => {
    const onChange = vi.fn();
    render(<HotkeyRecorder value="Command+Shift+V" onChange={onChange} />);
    await begin(); fireEvent.keyDown(input(), { key: "Escape", code: "Escape", metaKey }); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "dictation" });
    expect(onChange).not.toHaveBeenCalled();
    expect(trigger()).toHaveAccessibleName(/更改听写快捷键：.*(?:⌘|Ctrl).*(?:⇧|Shift).*V/);
  });

  it("ignores repeats and commits only after every key releases and native save ACK", async () => {
    const save = deferred(); invokeMock.mockResolvedValueOnce(undefined).mockReturnValueOnce(save.promise);
    const onChange = vi.fn(), onBusy = vi.fn();
    render(<HotkeyRecorder value="Fn" onChange={onChange} onCaptureBusyChange={onBusy} />);
    await begin(); pressCombination();
    fireEvent.keyDown(input(), { key: " ", code: "Space", metaKey: true, altKey: true, repeat: true });
    await flush(); expect(invokeMock).toHaveBeenCalledOnce();
    fireEvent.keyUp(window, { key: "Meta", code: "MetaLeft", altKey: true });
    fireEvent.keyUp(window, { key: " ", code: "Space", altKey: true }); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(2); expect(onChange).not.toHaveBeenCalled();
    fireEvent.keyUp(window, { key: "Alt", code: "AltLeft" }); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(2); expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("正在保存快捷键…");
    save.resolve(); await flush();
    expect(onChange).toHaveBeenCalledExactlyOnceWith("CmdOrControl+Alt+Space", { persist: false });
    expect(onBusy.mock.calls).toEqual([[true], [false]]);
    expect(screen.getByRole("status")).toHaveTextContent("已保存");
  });

  it("waits for modifiers even when their keydown was outside the input", async () => {
    const release = deferred(); invokeMock.mockResolvedValueOnce(undefined).mockReturnValueOnce(release.promise);
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin(); fireEvent.keyDown(input(), { key: "v", code: "KeyV", metaKey: true });
    fireEvent.keyUp(window, { key: "v", code: "KeyV", metaKey: true }); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(2); expect(onChange).not.toHaveBeenCalled();
    fireEvent.keyUp(window, { key: "Meta", code: "MetaLeft" }); await flush();
    release.resolve(); await flush();
    expect(onChange).toHaveBeenCalledWith("CmdOrControl+V", { persist: false });
  });

  it.each([false, true])("allows Tab and reverse Tab navigation: %s", async (shiftKey) => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin();
    expect(fireEvent.keyDown(input(), { key: "Tab", code: "Tab", shiftKey })).toBe(true);
    fireEvent.keyUp(window, { key: "Tab", code: "Tab" });
    const cancel = screen.getByRole("button", { name: "取消" }); act(() => cancel.focus());
    expect(cancel).toHaveFocus(); expect(input()).toBeVisible();
    expect(fireEvent.keyDown(cancel, { key: "Enter", code: "Enter" })).toBe(true); await flush();
    expect(onChange).not.toHaveBeenCalled(); expect(invokeMock).toHaveBeenCalledOnce();
  });

  it.each([
    { key: "a", code: "KeyA" }, { key: "Enter", code: "Enter" },
    { key: " ", code: "Space" }, { key: "A", code: "KeyA", shiftKey: true },
  ])("does not bind ordinary typing/navigation: $key", async (event) => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin(); fireEvent.keyDown(input(), event); fireEvent.keyUp(window, { key: event.key, code: event.code }); await flush();
    expect(onChange).not.toHaveBeenCalled(); expect(invokeMock).toHaveBeenCalledOnce();
    expect(screen.getByRole("status")).toHaveTextContent("请使用 ⌘、⌥ 或 ⌃");
  });

  it.each(["Meta", "Alt", "Control", "Shift"])("keeps standalone %s as a preview and offers explicit Fn", async (key) => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Command+Shift+V" onChange={onChange} />);
    await begin();
    for (let i = 0; i < 2; i++) {
      fireEvent.keyDown(input(), { key, code: `${key}Left`, metaKey: key === "Meta", altKey: key === "Alt", ctrlKey: key === "Control", shiftKey: key === "Shift" });
      fireEvent.keyUp(window, { key, code: `${key}Left` });
    }
    await flush(); expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("请再按一个键组成快捷键");
    fireEvent.click(screen.getByRole("button", { name: "使用 Fn" })); await flush();
    expect(onChange).toHaveBeenCalledWith("Fn", { persist: false });
  });

  it.each(optionalTargets)("allows clearing %s while leaving mode independent", async (target) => {
    const onChange = vi.fn();
    function Controlled() {
      const [value, setValue] = useState("Command+Shift+V");
      return <HotkeyRecorder value={value} captureTarget={target} onChange={(hotkey, options) => { setValue(hotkey); onChange(hotkey, options); }} />;
    }
    render(<Controlled />); await begin();
    if (["selected_action", "screen_action"].includes(target)) expect(screen.queryByRole("button", { name: "使用 Fn" })).not.toBeInTheDocument();
    fireEvent.keyDown(input(), { key: "Delete", code: "Delete" }); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: "", captureTarget: target });
    expect(onChange).toHaveBeenCalledExactlyOnceWith("", { persist: false });
    expect(trigger()).toHaveAccessibleName(/未设置/);
  });

  it("clears an optional shortcut from its idle clear action", async () => {
    const onChange = vi.fn();
    render(<HotkeyRecorder value="Command+Shift+V" captureTarget="selected_action" onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "清除选中文本快捷键" })); await flush();
    expect(invokeMock.mock.calls.map((call) => call[1])).toEqual([
      { suspended: true, capturedHotkey: null, captureTarget: "selected_action" },
      { suspended: false, capturedHotkey: "", captureTarget: "selected_action" },
    ]);
    expect(onChange).toHaveBeenCalledWith("", { persist: false });
  });

  it.each(["Backspace", "Delete"])("%s preserves the required dictation shortcut", async (key) => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin(); fireEvent.keyDown(input(), { key, code: key }); await flush();
    expect(onChange).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "dictation" });
  });

  it.each(["blur", "outside", "focus"])("cancels on %s and discards a held candidate", async (reason) => {
    const onChange = vi.fn(); render(<><HotkeyRecorder value="Fn" onChange={onChange} /><button>其他设置</button></>);
    await begin(); pressCombination();
    if (reason === "blur") fireEvent.blur(window);
    if (reason === "outside") fireEvent.pointerDown(screen.getByRole("button", { name: "其他设置" }));
    if (reason === "focus") act(() => screen.getByRole("button", { name: "其他设置" }).focus());
    await flush(); releaseCombination(); await flush();
    expect(onChange).not.toHaveBeenCalled(); expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "dictation" });
  });

  it.each(["saving", "cancelling"])("does not steal outside focus after %s completes", async (phase) => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    const complete = deferred(); invokeMock.mockResolvedValueOnce(undefined).mockReturnValueOnce(complete.promise);
    render(<><HotkeyRecorder value="Fn" onChange={vi.fn()} /><input aria-label="其他设置" /></>); await begin();
    if (phase === "saving") { pressCombination(); releaseCombination(); }
    else fireEvent.keyDown(input(), { key: "Escape", code: "Escape" });
    await flush(); const outside = screen.getByRole("textbox", { name: "其他设置" });
    fireEvent.pointerDown(outside); act(() => outside.focus());
    complete.resolve(); await flush(); expect(outside).toHaveFocus();
  });

  it("returns focus after an explicit cancel", async () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    render(<HotkeyRecorder value="Fn" onChange={vi.fn()} />); await begin();
    fireEvent.click(screen.getByRole("button", { name: "取消" })); await flush(); expect(trigger()).toHaveFocus();
  });

  it("accepts an in-group Fn click when WebKit reports no blur target", async () => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Command+Alt+Space" onChange={onChange} />);
    await begin(); fireEvent.blur(input(), { relatedTarget: null });
    fireEvent.click(screen.getByRole("button", { name: "使用 Fn" })); await flush();
    expect(onChange).toHaveBeenCalledExactlyOnceWith("Fn", { persist: false });
  });

  it("restores the default dictation shortcut through the same native transaction", async () => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin(); fireEvent.click(screen.getByRole("button", { name: "恢复默认快捷键" })); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: "CmdOrControl+Alt+Space", captureTarget: "dictation" });
    expect(onChange).toHaveBeenCalledExactlyOnceWith("CmdOrControl+Alt+Space", { persist: false });
  });

  it("hands a candidate to native when WebKit omits the ordinary keyup", async () => {
    const onChange = vi.fn(); render(<HotkeyRecorder value="Fn" onChange={onChange} />);
    await begin(); pressCombination();
    // Space/Alt releases were lost; Meta reports its old flag on release.
    fireEvent.keyUp(window, { key: "Meta", code: "MetaLeft", metaKey: true }); await flush();
    expect(onChange).toHaveBeenCalledExactlyOnceWith("CmdOrControl+Alt+Space", { persist: false });
  });

  it.each([
    ["save failed", "快捷键未能保存，原设置已保留。请重试。"],
    ["hotkey conflicts with another shortcut", "这个快捷键已用于其他功能，请换一个组合键。"],
    ["failed to register global hotkey", "这个快捷键无法使用，可能已被系统或其他 App 占用。请换一个组合键。"],
    ["failed to register global hotkey；原快捷键恢复失败", "原快捷键未能恢复，请重新录制或重启 VoiceFlow。"],
  ])("keeps the displayed binding after failure and offers retry: %s", async (reason, message) => {
    invokeMock.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error(reason));
    const onChange = vi.fn(); render(<HotkeyRecorder value="Command+Shift+V" onChange={onChange} />);
    await begin(); pressCombination(); releaseCombination(); await flush();
    expect(onChange).not.toHaveBeenCalled(); expect(trigger()).toHaveAccessibleName(/更改听写快捷键：.*(?:⌘|Ctrl).*(?:⇧|Shift).*V/);
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    fireEvent.click(screen.getByRole("button", { name: "重试" })); await flush();
    expect(input()).toHaveAttribute("aria-busy", "false"); expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("serializes a new page's capture behind the departed page's pending save", async () => {
    const save = deferred(); invokeMock.mockResolvedValueOnce(undefined).mockReturnValueOnce(save.promise);
    const oldChange = vi.fn(), newChange = vi.fn();
    const oldView = render(<HotkeyRecorder value="Fn" onChange={oldChange} />);
    await begin(); pressCombination(); releaseCombination(); await flush(); oldView.unmount();
    render(<HotkeyRecorder value="" captureTarget="screen_action" onChange={newChange} />);
    fireEvent.click(trigger()); await flush();
    expect(invokeMock).toHaveBeenCalledTimes(2); expect(input()).toHaveAttribute("aria-busy", "true");
    save.resolve(); await flush();
    expect(oldChange).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: true, capturedHotkey: null, captureTarget: "screen_action" });
    expect(input()).toHaveAttribute("aria-busy", "false");
    fireEvent.keyDown(input(), { key: "Escape", code: "Escape" }); await flush();
    expect(invokeMock).toHaveBeenLastCalledWith("set_hotkeys_suspended", { suspended: false, capturedHotkey: null, captureTarget: "screen_action" });
    expect(newChange).not.toHaveBeenCalled();
  });

  it("skips a queued pause if the new recorder cancels before it starts", async () => {
    const save = deferred(); invokeMock.mockResolvedValueOnce(undefined).mockReturnValueOnce(save.promise);
    const oldView = render(<HotkeyRecorder value="Fn" onChange={vi.fn()} />);
    await begin(); pressCombination(); releaseCombination(); await flush(); oldView.unmount();
    render(<HotkeyRecorder value="" captureTarget="screen_action" onChange={vi.fn()} />);
    fireEvent.click(trigger()); await flush(); fireEvent.click(screen.getByRole("button", { name: "取消" }));
    save.resolve(); await flush(); expect(invokeMock).toHaveBeenCalledTimes(2); expect(trigger()).toBeVisible();
  });
});
