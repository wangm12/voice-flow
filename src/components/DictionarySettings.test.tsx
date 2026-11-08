import { selectOption } from "../test/selectOption";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Settings } from "../types/settings";
import { DictionarySettings } from "./DictionarySettings";
import { auditPair, auditSnapshot } from "../test/dictionaryLearningAuditFixtures";

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

  it("adds manual ownership for a learned word already in a full dictionary", async () => {
    const dictionary = ["VoiceFlow", ...Array.from({ length: 255 }, (_, index) => `term-${index}`)];
    invokeMock.mockImplementation(async (command) => command === "list_learn_pairs"
      ? [{ ...promotedPair, after_surface: "VoiceFlow" }] : []);
    render(<DictionarySettings settings={{ ...settings, dictionary }} save={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "添加个人词典词条" });
    fireEvent.change(input, { target: { value: "voiceflow" } });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_dictionary_entries", { words: ["VoiceFlow"] }));
    expect(input).toHaveValue("");
    expect(await screen.findByText("已有词条已登记为手动词条。")).toBeInTheDocument();
  });

  it("imports an existing learned word as manual ownership even when the dictionary is full", async () => {
    const dictionary = ["VoiceFlow", ...Array.from({ length: 255 }, (_, index) => `term-${index}`)];
    invokeMock.mockImplementation(async (command) => command === "list_learn_pairs"
      ? [{ ...promotedPair, after_surface: "VoiceFlow" }] : []);
    const { container } = render(<DictionarySettings settings={{ ...settings, dictionary }} save={vi.fn()} />);
    const input = container.querySelector('input[type="file"]');
    expect(input).not.toBeNull();
    fireEvent.change(input!, { target: { files: [new File(["voiceflow"], "terms.txt", { type: "text/plain" })] } });

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_dictionary_entries", { words: ["VoiceFlow"] }));
    expect(await screen.findByText(/已登记个人词条：1 条/)).toBeInTheDocument();
  });

  it("reports a partial personal dictionary sync after manual ownership was recorded", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "add_dictionary_entries") throw "learning_dictionary_partial:settings_save_failed";
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "添加个人词典词条" }), { target: { value: "OpenAI" } });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("词条来源已记录，但个人词典未能同步。再次添加此词条可重试。");
  });

  it("reports a partial personal dictionary sync after importing a file", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "add_dictionary_entries") throw "learning_dictionary_partial:settings_save_failed";
      return [];
    });
    const { container } = render(<DictionarySettings settings={settings} save={vi.fn()} />);
    const input = container.querySelector('input[type="file"]');
    fireEvent.change(input!, { target: { files: [new File(["OpenAI"], "terms.txt", { type: "text/plain" })] } });

    expect(await screen.findByRole("alert")).toHaveTextContent("词条来源已记录，但个人词典未能同步。请重新导入以重试。");
  });

  it("reports partial dictionary synchronization for a learned rule action", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [promotedPair];
      if (command === "undo_learn_pair") throw "learning_dictionary_partial:settings_save_failed";
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    await screen.findByText("知呼 → 知乎");
    fireEvent.click(screen.getByRole("button", { name: "忘记" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
  });

  it("shows the dictionary capacity message when backend promotion is rejected", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [pendingPair];
      if (command === "promote_learn_pair") throw "dictionary_capacity_or_invalid_word";
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    expect(await screen.findByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "确认" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("词条无效或已达到上限。");
    expect(screen.getByRole("group", { name: "知呼 → 知乎" })).toBeInTheDocument();
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
    expect(screen.getByText(/普通词需 3 次、人名需 2 次不同听写中的纠正/)).toBeInTheDocument();
    expect(screen.getByText(/继续打字不会学习/)).toBeInTheDocument();
    expect(await screen.findByText(/还没有已生效替换/)).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(save).toHaveBeenCalledWith({ dictionary_learn_enabled: false });
  });

  it("keeps same-field local-edit observation off until the user opts in", () => {
    const save = vi.fn();
    render(<DictionarySettings settings={settings} save={save} />);

    const toggle = screen.getByRole("switch", { name: "本机观察插入后的改动" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText(/只学习唯一插入片段内的改动/)).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(save).toHaveBeenCalledWith({ local_edit_learning_enabled: true });
  });

  it("adds a reversible before→after rule for an explicitly selected running native app", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_manual_learn_applications") {
        return [{ bundle_id: "com.openai.codex", label: "Codex" }];
      }
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    fireEvent.change(await screen.findByRole("textbox", { name: "听写识别为" }), { target: { value: "L O M" } });
    fireEvent.change(screen.getByRole("textbox", { name: "应替换为" }), { target: { value: "LLM" } });
    selectOption(await screen.findByRole("combobox", { name: "仅在此 App 生效" }), "com.openai.codex");
    fireEvent.click(screen.getByRole("button", { name: "添加专用纠正" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_manual_scoped_learn_pair", {
      beforeSurface: "L O M",
      afterSurface: "LLM",
      bundleId: "com.openai.codex",
    }));
    expect(screen.getByText(/规则只在所选 App 生效，不改全局个人词典，也不改历史记录/)).toBeInTheDocument();
  });

  it("explains when a manual correction already exists under another scope", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_manual_learn_applications") {
        return [{ bundle_id: "com.openai.codex", label: "Codex" }];
      }
      if (command === "add_manual_scoped_learn_pair") throw "manual_pair_scope_conflict";
      return [];
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    fireEvent.change(await screen.findByRole("textbox", { name: "听写识别为" }), { target: { value: "L O M" } });
    fireEvent.change(screen.getByRole("textbox", { name: "应替换为" }), { target: { value: "LLM" } });
    selectOption(await screen.findByRole("combobox", { name: "仅在此 App 生效" }), "com.openai.codex");
    fireEvent.click(screen.getByRole("button", { name: "添加专用纠正" }));
    expect(await screen.findByText(/同一纠正已在其他范围登记/)).toBeInTheDocument();
  });

  it("opens the authoritative audit without replaying controls or multiplying shared term usage", async () => {
    const save = vi.fn();
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [promotedPair, { ...promotedPair, pair_key: "second-rule", before_surface: "知乎" }];
      if (command === "list_learned_term_usage") return [{ word: "知乎", replacement_runs: 7, last_replaced_at: "2026-10-15 00:00:00" }];
      if (command === "get_dictionary_learning_audit") return auditSnapshot({ pairs: [auditPair({ before_surface: "知呼", after_surface: "知乎", record_state: "db_promoted" }), auditPair({ pair_ref: "55555555-5555-4555-8555-555555555555", pair_key: "second-rule", before_surface: "知乎", after_surface: "知乎", record_state: "db_promoted" })] });
      return [];
    });
    render(<DictionarySettings settings={{ ...settings, dictionary: ["知乎"] }} save={save} />);
    await screen.findByText("知呼 → 知乎");
    expect(invokeMock.mock.calls.some(([command]) => command === "get_dictionary_learning_audit")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: /学习统计/ }));
    expect(screen.getAllByText(/7 次本机处理/)).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "学习审查：来源、原因与保存步骤" }));
    await screen.findByLabelText("学习审查读取结果");
    expect(screen.getAllByText(/7 次本机处理/)).toHaveLength(1);
    expect(save).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => ["promote_learn_pair", "undo_learn_pair", "ignore_learn_pair", "get_context_snapshot", "show_permission_guide"].includes(command as string))).toBe(false);
    expect(invokeMock.mock.calls.filter(([command]) => command === "list_learned_term_usage")).toHaveLength(1);
  });

  it("teaches at the real dictionary entry without adding its example or discarding a typed term", async () => {
    const save = vi.fn();
    const onOpenHistory = vi.fn();
    const onOpenPreferences = vi.fn();
    render(<DictionarySettings settings={settings} save={save} onOpenHistory={onOpenHistory} onOpenPreferences={onOpenPreferences} />);
    const input = screen.getByRole("textbox", { name: "添加个人词典词条" });
    fireEvent.change(input, { target: { value: "保留我的草稿" } });
    fireEvent.click(screen.getByRole("button", { name: "词典与纠错：从添加到真正使用" }));
    expect(screen.getByText(/同一听写只计一次/)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "添加自己的术语" }));
    expect(input).toHaveFocus();
    expect(input).toHaveValue("保留我的草稿");
    expect(save).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => command === "add_dictionary_entries" || command === "promote_learn_pair")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "去历史纠正文字" }));
    fireEvent.click(screen.getByRole("button", { name: "查看 App 学习与风格规则" }));
    expect(onOpenHistory).toHaveBeenCalledOnce();
    expect(onOpenPreferences).toHaveBeenCalledOnce();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("list_learn_pairs"));
  });

  it("shows the actual style pair and future provider approval before the existing confirmation", async () => {
    invokeMock.mockImplementation(async (command) => command === "list_style_drafts" ? [styleDraft] : []);
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    await screen.findByText(/wechat · 少用句号 · 你好/);
    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("并允许用于后续整理请求");
    expect(dialog).toHaveTextContent("现在不会发送");
    expect(dialog).toHaveTextContent("示例输入：你好。 → 期望输出：你好");
    expect(invokeMock.mock.calls.some(([command]) => command === "confirm_style_draft")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.some(([command]) => command === "confirm_style_draft")).toBe(false);
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

  it("shows the historical correction count without presenting it as the independent threshold", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [pendingPair];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);

    const pendingRow = await screen.findByRole("group", { name: "知呼 → 知乎" });
    expect(pendingRow).toHaveTextContent("2 次纠正记录");
    expect(pendingRow).not.toHaveTextContent("2/3");
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

  it("lists promoted dictionary words with deletion and preserves their separate rule Forget action", async () => {
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
    expect(wordList).toHaveTextContent("知乎");
    expect(wordList).toHaveTextContent("李明");

    fireEvent.click(within(wordList!).getByRole("button", { name: "删除 知乎" }));
    const deleteDialog = screen.getByRole("dialog", { name: "删除词条" });
    fireEvent.click(within(deleteDialog).getByRole("button", { name: "删除词条" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("remove_dictionary_word", { word: "知乎" }),
    );

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

  it("refreshes failed App choices in place without losing the correction draft", async () => {
    const base = invokeMock.getMockImplementation()!;
    let failed = true;
    invokeMock.mockImplementation((command, args, options) => {
      if (command === "get_manual_learn_applications") return failed ? Promise.reject(new Error("unavailable")) : Promise.resolve([{ bundle_id: "com.apple.TextEdit", label: "TextEdit" }]);
      return base(command, args, options);
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);
    await screen.findByText("无法读取当前运行的 App；请重试后再添加。");
    fireEvent.change(screen.getByRole("textbox", { name: "听写识别为" }), { target: { value: "wrong" } });
    failed = false;
    fireEvent.click(screen.getByRole("button", { name: "刷新 App 列表" }));
    const control = screen.getByRole("combobox", { name: "仅在此 App 生效" });
    await waitFor(() => expect(control).not.toBeDisabled());
    selectOption(control, "com.apple.TextEdit");
    expect(control).toHaveValue("com.apple.TextEdit");
    expect(screen.getByRole("textbox", { name: "听写识别为" })).toHaveValue("wrong");
    expect(screen.queryByText("无法读取当前运行的 App；请重试后再添加。")).not.toBeInTheDocument();
  });

});
