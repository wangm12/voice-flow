import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Settings } from "../types/settings";
import { DictionarySettings } from "./DictionarySettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: () => Promise.resolve(() => undefined),
  }),
}));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

const settings: Settings = {
  schema_version: 4,
  api_key_configured: true,
  api_key_hint: "gsk_…abcd",
  asr_model: "whisper-large-v3-turbo",
  cleanup_model: "openai/gpt-oss-20b",
  language: "auto",
  ui_language: "zh",
  theme: "system",
  dictionary: [],
  chunk_threshold_secs: 25,
  chunk_length_secs: 35,
  long_output_mode: "paste",
  delivery_policy: "auto",
  keep_audio_days: 7,
  keep_history_days: 90,
  onboarded: true,
  cleanup_enabled: true,
  show_tray_icon: true,
  hotkey: "CmdOrControl+Shift+Space",
  activation_mode: "tap",
  context_enabled: true,
  browser_access_enabled: false,
  context_mappings: [],
  writing_modes: [{ id: "general", label: "通用", family: "general", prompt: "保持原意。", builtin: true }],
  snippets: [],
  output_mode: "auto",
  translation_target_language: "en",
  input_device: "",
};

const pendingPair = {
  pair_key: "知呼\u001e知乎",
  before_surface: "知呼",
  after_surface: "知乎",
  hits: 2,
  promoted: false,
  last_at: "2026-08-22 00:00:00",
  promote_hits: 3,
};

const promotedPair = {
  pair_key: "知呼\u001e知乎",
  before_surface: "知呼",
  after_surface: "知乎",
  hits: 3,
  promoted: true,
  last_at: "2026-08-22 00:00:00",
  promote_hits: 3,
  pinned: false,
};

const promotedName = {
  pair_key: "李铭\u001e李明",
  before_surface: "李铭",
  after_surface: "李明",
  hits: 2,
  promoted: true,
  last_at: "2026-08-22 00:00:00",
  promote_hits: 2,
  pinned: false,
};

const styleDraft = {
  draft_key: "wechat\u001efewer_periods",
  mapping_id: "wechat",
  style_key: "fewer_periods",
  excerpt: "你好",
  before_excerpt: "你好。",
  after_excerpt: "你好",
};

function emitLearningChange() {
  const callback = listenMock.mock.calls.find(([event]) => event === "learn_pairs://changed")?.[1];
  expect(callback).toBeDefined();
  act(() => callback!({ event: "learn_pairs://changed", id: 1, payload: null }));
}

describe("DictionarySettings", () => {
  it("coalesces repeated Enter presses while retaining a newly typed word", async () => {
    let complete!: () => void;
    invokeMock.mockImplementation((command) => command === "add_dictionary_entries"
      ? new Promise<void>((resolve) => { complete = resolve; }) : Promise.resolve([]));
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "添加个人词典词条" });
    fireEvent.change(input, { target: { value: "第一词条" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(invokeMock.mock.calls.filter(([command]) => command === "add_dictionary_entries")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "添加中…" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "接下来添加的词条" } });
    await act(async () => complete());
    expect(input).toHaveValue("接下来添加的词条");
    expect(screen.getByRole("button", { name: "添加" })).toBeEnabled();
  });
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
    listenMock.mockResolvedValue(() => undefined);
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
  });

  it("defaults dictionary learning on and can turn it off", async () => {
    const save = vi.fn();
    render(<DictionarySettings settings={settings} save={save} />);

    const toggle = screen.getByRole("switch", { name: "学习词条" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/默认第 3 次静默纠正，人名第 2 次/)).toBeInTheDocument();
    expect(screen.getByText(/继续打字不会学习/)).toBeInTheDocument();
    expect(await screen.findByText(/还没有已生效替换/)).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(save).toHaveBeenCalledWith({ dictionary_learn_enabled: false });
  });

  it.each(["done", "copied", "history", "unverified", "degraded"])("refreshes actual replacement usage after %s completion", async (state) => {
    let runs = 2;
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [promotedPair, { ...pendingPair, pair_key: "other", after_surface: "另一个词" }];
      if (command === "list_learned_term_usage") return [{ word: "知乎", replacement_runs: runs, last_replaced_at: "2026-10-02 04:00:00" }, { word: "已忘记", replacement_runs: 20, last_replaced_at: "2026-10-01 00:00:00" }];
      return [];
    });
    render(<DictionarySettings settings={{ ...settings, dictionary: ["知乎"], dictionary_learn_enabled: false }} save={vi.fn()} />);
    await screen.findByText(/2 次本机处理/);
    const feedback = screen.getByLabelText("学习反馈");
    expect(within(feedback).getByText("已用于本机纠正").parentElement).toHaveTextContent("1");
    expect(within(feedback).getByText("待确认").parentElement).toHaveTextContent("1");
    expect(within(feedback).getByText("已生效替换").parentElement).toHaveTextContent("1");
    expect(screen.queryByText("已忘记")).not.toBeInTheDocument();
    expect(screen.getByText(/自动观察已关闭/)).toBeInTheDocument();
    runs = 3;
    const complete = listenMock.mock.calls.find(([name]) => name === "dictation://state")?.[1];
    act(() => { complete?.({ payload: { state } } as never); });
    expect(await screen.findByText(/3 次本机处理/)).toBeInTheDocument();
  });

  it("does not show missing usage as zero when the feedback query fails", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [promotedPair];
      if (command === "list_learned_term_usage") throw new Error("synthetic unavailable usage");
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent(/未能读取/);
    expect(within(screen.getByLabelText("学习反馈")).getByText("已用于本机纠正").parentElement).toHaveTextContent("—");
  });

  it("shows a pending pair as before → after · 2/3", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [pendingPair];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);

    expect(await screen.findByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();
    expect(listenMock).toHaveBeenCalledWith("learn_pairs://changed", expect.any(Function));
  });

  it("shows a style draft for human review", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [];
      if (command === "list_style_drafts") {
        return [{
          draft_key: "wechat\u001efewer_periods",
          mapping_id: "wechat",
          style_key: "fewer_periods",
          excerpt: "你好",
          before_excerpt: "你好。",
          after_excerpt: "你好",
        }];
      }
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByText(/少用句号/)).toBeInTheDocument();
  });

  it("confirms and ignores pending pairs", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [pendingPair];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("promote_learn_pair", {
        pairKey: pendingPair.pair_key,
        beforeSurface: "知呼",
        afterSurface: "知乎",
      }),
    );

    await waitFor(() => expect(screen.getByRole("button", { name: "忽略" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "忽略" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("ignore_learn_pair", {
        pairKey: pendingPair.pair_key,
      }),
    );
  });

  it("shows promoted replacements and can forget them", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [promotedPair, promotedName];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return ["知乎"];
      return undefined;
    });
    render(
      <DictionarySettings
        settings={{ ...settings, dictionary: ["知乎", "李明", "手动导入"] }}
        save={vi.fn()}
      />,
    );

    expect(await screen.findByText("知呼 → 知乎")).toBeInTheDocument();
    const nameRow = screen.getByText("李铭 → 李明").closest("div");
    expect(nameRow).toHaveTextContent("2 次");
    expect(nameRow).toHaveTextContent("人名");
    expect(screen.getByText("手动导入")).toBeInTheDocument();
    expect(screen.queryByText(/还没有已生效替换/)).not.toBeInTheDocument();
    const wordList = screen.getByText("词条列表").closest("section");
    expect(wordList).toHaveTextContent("手动导入");
    expect(wordList).not.toHaveTextContent("知乎");
    expect(wordList).not.toHaveTextContent("李明");

    fireEvent.click(screen.getAllByRole("button", { name: "忘记" })[0]);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("undo_learn_pair", {
        pairKey: promotedPair.pair_key,
      }),
    );
  });

  it("shows initial read failure with retry instead of a successful empty state", async () => {
    let failing = true;
    invokeMock.mockImplementation(async (command) => {
      if (command.startsWith("list_")) {
        if (failing) throw new Error("synthetic unavailable database");
        return [];
      }
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent(/未能读取/);
    expect(screen.getByText("词典学习数据尚未读取。")).toBeInTheDocument();
    expect(screen.queryByText(/还没有已生效替换/)).not.toBeInTheDocument();
    failing = false;
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByText(/还没有已生效替换/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "list_learn_pairs")).toHaveLength(2);
  });

  it("retains successfully loaded rows and pin state if a later reload fails", async () => {
    let failing = false;
    invokeMock.mockImplementation(async (command) => {
      if (failing && command.startsWith("list_")) throw new Error("synthetic reload failure");
      if (command === "list_learn_pairs") return [pendingPair, promotedName];
      if (command === "list_style_drafts") return [styleDraft];
      if (command === "list_pinned_terms") return ["手动导入"];
      return undefined;
    });
    render(<DictionarySettings settings={{ ...settings, dictionary: ["手动导入"] }} save={vi.fn()} />);
    expect(await screen.findByText("李铭 → 李明")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消置顶 手动导入" })).toBeInTheDocument();
    failing = true;
    emitLearningChange();

    expect(await screen.findByRole("alert")).toHaveTextContent(/已有内容已保留/);
    expect(screen.getByText("李铭 → 李明")).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();
    expect(screen.getByText(/wechat · 少用句号 · 你好/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消置顶 手动导入" })).toBeInTheDocument();
    expect(screen.queryByText(/还没有已生效替换/)).not.toBeInTheDocument();
  });

  it.each(["list_learn_pairs", "list_style_drafts", "list_pinned_terms"])(
    "preserves the failed %s resource while updating other successful resources",
    async (failedCommand) => {
      let reloading = false;
      invokeMock.mockImplementation(async (command) => {
        if (reloading && command === failedCommand) throw new Error("synthetic one-resource failure");
        if (command === "list_learn_pairs") return reloading ? [promotedName] : [promotedPair];
        if (command === "list_style_drafts") return [{ ...styleDraft, excerpt: reloading ? "新版口癖" : "你好" }];
        if (command === "list_pinned_terms") return reloading ? [] : ["手动导入"];
        return undefined;
      });
      render(<DictionarySettings settings={{ ...settings, dictionary: ["手动导入"] }} save={vi.fn()} />);
      expect(await screen.findByText("知呼 → 知乎")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "取消置顶 手动导入" })).toBeInTheDocument();
      reloading = true;
      emitLearningChange();

      expect(await screen.findByRole("alert")).toHaveTextContent(/未能读取/);
      expect(screen.getByText(failedCommand === "list_learn_pairs" ? "知呼 → 知乎" : "李铭 → 李明")).toBeInTheDocument();
      expect(screen.getByText(failedCommand === "list_style_drafts" ? /wechat · 少用句号 · 你好/ : /wechat · 少用句号 · 新版口癖/)).toBeInTheDocument();
      expect(screen.getByRole("button", { name: failedCommand === "list_pinned_terms" ? "取消置顶 手动导入" : "置顶 手动导入" })).toBeInTheDocument();
    },
  );

  it("keeps a failed learning action available until retry succeeds", async () => {
    let promoted = false;
    let attempts = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return promoted ? [promotedPair] : [pendingPair];
      if (command === "list_style_drafts" || command === "list_pinned_terms") return [];
      if (command === "promote_learn_pair") {
        attempts += 1;
        if (attempts === 1) throw new Error("synthetic write failure");
        promoted = true;
      }
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("词典操作未完成，请重试。");
    expect(screen.getByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    expect(await screen.findByText("知呼 → 知乎")).toBeInTheDocument();
    expect(screen.queryByText("知呼 → 知乎 · 2/3")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(attempts).toBe(2);
  });

  it("keeps the style confirmation dialog open after a failed action for retry", async () => {
    let attempts = 0;
    let confirmed = false;
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs" || command === "list_pinned_terms") return [];
      if (command === "list_style_drafts") return confirmed ? [] : [styleDraft];
      if (command === "confirm_style_draft") {
        attempts += 1;
        if (attempts === 1) throw new Error("synthetic style write failure");
        confirmed = true;
      }
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByText(/wechat · 少用句号 · 你好/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "确认" }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "confirm_style_draft")).toHaveLength(1));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
    expect(screen.getByRole("dialog")).toHaveTextContent("确认口癖样例");
    // Wait for the failure to finish before retrying; the dialog remains the entry point.
    await waitFor(() => expect(screen.getByText("词典操作未完成，请重试。")).toBeInTheDocument());
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "确认" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByText(/wechat · 少用句号 · 你好/)).not.toBeInTheDocument();
    expect(attempts).toBe(2);
  });

});
