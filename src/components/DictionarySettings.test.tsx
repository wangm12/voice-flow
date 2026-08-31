import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  activation_mode: "hybrid",
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

describe("DictionarySettings", () => {
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

  it("defaults dictionary learning on and can turn it off", () => {
    const save = vi.fn();
    render(<DictionarySettings settings={settings} save={save} />);

    const toggle = screen.getByRole("switch", { name: "学习词条" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/默认第 3 次静默纠正，人名第 2 次/)).toBeInTheDocument();
    expect(screen.getByText(/继续打字不会学习/)).toBeInTheDocument();
    expect(screen.getByText(/还没有学到替换/)).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(save).toHaveBeenCalledWith({ dictionary_learn_enabled: false });
  });

  it("shows a pending pair as before → after · 2/3", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "list_learn_pairs") return [pendingPair];
      if (command === "list_style_drafts") return [];
      if (command === "list_pinned_terms") return [];
      return undefined;
    });
    render(<DictionarySettings settings={settings} save={vi.fn()} />);

    expect(await screen.findByText("知呼 → 知乎 · 2/3")).toBeInTheDocument();
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
    expect(await screen.findByText("知呼 → 知乎 · 2/3")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "确认" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("promote_learn_pair", {
        pairKey: pendingPair.pair_key,
        beforeSurface: "知呼",
        afterSurface: "知乎",
      }),
    );

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
    expect(screen.queryByText(/还没有学到替换/)).not.toBeInTheDocument();
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
});
