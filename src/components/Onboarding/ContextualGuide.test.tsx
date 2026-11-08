import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { I18nProvider } from "../../lib/i18n";
import { onDemandGuideIds, parseGuideAcknowledgements, type OnDemandGuideId } from "../../lib/onboardingGuides";
import { ContextualGuide } from "./ContextualGuide";
import { ContextLesson, DictionaryLesson, SceneLesson, SelectedLesson } from "./ContextualLessons";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const receipt = (acknowledged: OnDemandGuideId[] = []) => onDemandGuideIds.map((guide_id) => ({ guide_id, current_version: 1, acknowledged_version: acknowledged.includes(guide_id) ? 1 : null }));

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(receipt());
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("native guide receipt boundary", () => {
  it("preserves independent acknowledgements and rejects missing, duplicate or malformed identities", () => {
    const rows = receipt(["context_screen"]);
    expect(parseGuideAcknowledgements(rows)).toEqual(rows);
    expect(parseGuideAcknowledgements(rows.slice(1))).toBeNull();
    expect(parseGuideAcknowledgements([...rows.slice(0, 3), rows[0]])).toBeNull();
    expect(parseGuideAcknowledgements(rows.map((row, index) => index ? row : { ...row, current_version: "1" }))).toBeNull();
    expect(parseGuideAcknowledgements(rows.map((row, index) => index ? row : { ...row, acknowledged_version: false }))).toBeNull();
  });
});

describe("optional contextual guide", () => {
  it("opens first-use inline without commands or focus, closes without acknowledgement, and can reopen", async () => {
    const read = vi.fn();
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    render(<><input aria-label="Current draft" /><ContextualGuide guideId="dictionary_learning" title="Dictionary guide" firstUse><button onClick={read}>Existing action</button></ContextualGuide></>);
    const draft = screen.getByRole("textbox", { name: "Current draft" });
    draft.focus();
    const trigger = screen.getByRole("button", { name: "Dictionary guide" });
    await waitFor(() => expect(trigger).toHaveAttribute("aria-expanded", "true"));
    expect(draft).toHaveFocus();
    const collapse = screen.getByRole("button", { name: "收起说明" });
    collapse.focus();
    fireEvent.click(collapse);
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Existing action" })).not.toBeInTheDocument();
    fireEvent.click(trigger);
    expect(screen.getByRole("button", { name: "Existing action" })).toBeVisible();
    expect(read).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["get_guide_acknowledgements"]);
  });

  it("does not let a late initial read undo the user's explicit close", async () => {
    let resolve!: (value: unknown) => void;
    invokeMock.mockImplementation(() => new Promise((done) => { resolve = done; }));
    render(<SelectedLesson />);
    const trigger = screen.getByRole("button", { name: "选中文本与翻译：先选来源，再确认结果" });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("button", { name: "收起说明" }));
    await act(async () => resolve(receipt()));
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(invokeMock).not.toHaveBeenCalledWith("acknowledge_guide", expect.anything());
  });

  it("keeps help collapsed by default and acknowledges only the explicitly opened guide", async () => {
    let rows = receipt();
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "acknowledge_guide") {
        expect(args).toEqual({ guideId: "dictionary_learning", version: 1 });
        rows = receipt(["dictionary_learning"]);
      }
      return rows;
    });
    const view = render(<DictionaryLesson />);
    const title = "词典与纠错：从添加到真正使用";
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_guide_acknowledgements"));
    expect(screen.getByRole("button", { name: title })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: title }));
    fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    await waitFor(() => expect(screen.getByRole("button", { name: title })).toHaveAttribute("aria-expanded", "false"));
    view.unmount();
    render(<><DictionaryLesson /><ContextLesson /></>);
    await waitFor(() => expect(screen.getByRole("button", { name: "上下文与看屏幕：分别选择来源和用途" })).toHaveAttribute("aria-expanded", "false"));
    expect(screen.getByRole("button", { name: title })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: title }));
    expect(screen.getByText(/同一听写只计一次/)).toBeVisible();
  });

  it("retains failed acknowledgement as unconfirmed and retries the same native version", async () => {
    let fail = true;
    invokeMock.mockImplementation(async (command) => {
      if (command === "acknowledge_guide") {
        if (fail) throw new Error("disk unavailable");
        return receipt(["scene_preferences"]);
      }
      return receipt();
    });
    render(<SceneLesson />);
    fireEvent.click(screen.getByRole("button", { name: "语气与个人偏好：草稿、保存和批准" }));
    await screen.findByRole("button", { name: "知道了" });
    fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("未保存");
    expect(screen.getByRole("button", { name: "语气与个人偏好：草稿、保存和批准" })).toHaveAttribute("aria-expanded", "true");
    fail = false;
    fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "语气与个人偏好：草稿、保存和批准" })).toHaveAttribute("aria-expanded", "false"));
    expect(invokeMock.mock.calls.filter(([command]) => command === "acknowledge_guide")).toHaveLength(2);
  });

  it("does not steal focus from another control when an acknowledgement finishes", async () => {
    let resolve!: (value: unknown) => void;
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    invokeMock.mockImplementation((command) => command === "acknowledge_guide" ? new Promise((done) => { resolve = done; }) : Promise.resolve(receipt()));
    render(<><SelectedLesson /><input aria-label="Other work" /></>);
    fireEvent.click(screen.getByRole("button", { name: "选中文本与翻译：先选来源，再确认结果" }));
    fireEvent.click(await screen.findByRole("button", { name: "知道了" }));
    const other = screen.getByRole("textbox", { name: "Other work" });
    other.focus();
    await act(async () => resolve(receipt(["selected_translation"])));
    expect(other).toHaveFocus();
  });

  it("does not acknowledge a native content version that this UI cannot teach", async () => {
    invokeMock.mockResolvedValue(receipt().map((row) => ({ ...row, current_version: 2 })));
    render(<SelectedLesson />);
    fireEvent.click(screen.getByRole("button", { name: "选中文本与翻译：先选来源，再确认结果" }));
    fireEvent.click(await screen.findByRole("button", { name: "知道了" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("未保存");
    expect(invokeMock.mock.calls.some(([command]) => command === "acknowledge_guide")).toBe(false);
  });

  it("keeps screen help optional in the hotkey entry and invokes navigation only on an explicit action", async () => {
    const permissions = vi.fn();
    render(<ContextLesson firstUse={false} onOpenPermissions={permissions} />);
    const trigger = screen.getByRole("button", { name: "上下文与看屏幕：分别选择来源和用途" });
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_guide_acknowledgements"));
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(trigger);
    expect(permissions).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "查看系统权限（不自动申请）" }));
    expect(permissions).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.every(([command]) => command === "get_guide_acknowledgements")).toBe(true);
  });

  it("offers English instructions without a provider or feature request", async () => {
    render(<I18nProvider initialLanguage="en"><SelectedLesson /></I18nProvider>);
    fireEvent.click(screen.getByRole("button"));
    await screen.findByRole("button", { name: "Got it" });
    expect(screen.getByText(/Fixed practice source \(example only\)/)).toBeVisible();
    expect(screen.getByText(/do not send messages or press Enter/)).toBeVisible();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["get_guide_acknowledgements"]);
  });
});
