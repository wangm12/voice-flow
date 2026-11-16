import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { LearningTransferSettings } from "./LearningTransferSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const preview = {
  status: "preview" as const,
  previewId: "preview-1",
  expiresAtMs: 1_900_000_000_000,
  sourceSummary: { globalWordCount: 3, pairCount: 1, preferenceCount: 3 },
  items: [
    {
      itemId: "word-ready",
      kind: "global_word" as const,
      summary: "VoiceFlow",
      scopeSummary: "全局个人词条",
      status: "ready" as const,
      defaultAction: "import" as const,
      choiceOptions: [],
      reason: "global_rule_owner_will_be_added",
      details: { word: "VoiceFlow", existingValue: "rule_owner" },
    },
    {
      itemId: "pair-conflict",
      kind: "word_pair" as const,
      summary: "知呼 → 知乎",
      scopeSummary: "全局 · 通用任务",
      status: "conflict" as const,
      defaultAction: "keep_local" as const,
      choiceOptions: ["keep_local", "replace"] as const,
      reason: "目标范围已存在另一条纠正",
      details: { before: "知呼", after: "知乎", existingValue: "知乎企业版" },
    },
    {
      itemId: "word-conflict",
      kind: "global_word" as const,
      summary: "OpenAI",
      scopeSummary: "全局个人词条",
      status: "conflict" as const,
      defaultAction: "keep_local" as const,
      choiceOptions: ["keep_local", "import_global"] as const,
    },
    {
      itemId: "word-already-present",
      kind: "global_word" as const,
      summary: "VoiceFlow",
      scopeSummary: "全局个人词条",
      status: "already_present" as const,
      defaultAction: "none" as const,
      choiceOptions: [] as const,
    },
    {
      itemId: "preference-ready",
      kind: "writing_preference" as const,
      summary: "GreetingTone: RespectfulGreeting",
      scopeSummary: "DeveloperCollaboration · bundle com.example.writer · selected task ProjectUpdate",
      willCreateMapping: true,
      status: "ready" as const,
      defaultAction: "import" as const,
      choiceOptions: [] as const,
      details: { taskId: "project_update" as const, dimension: "greeting_tone" as const, value: "respectful_greeting" as const },
    },
    {
      itemId: "preference-conflict",
      kind: "writing_preference" as const,
      summary: "GreetingTone: ConversationalGreeting",
      scopeSummary: "WorkChat · selected task WorkChat",
      status: "conflict" as const,
      defaultAction: "keep_local" as const,
      choiceOptions: ["keep_local", "replace"] as const,
      reason: "preference_conflict",
      details: {
        taskId: "work_chat" as const,
        dimension: "greeting_tone" as const,
        value: "conversational_greeting" as const,
        existingValue: "RespectfulGreeting (Paused)",
      },
    },
    {
      itemId: "preference-blocked",
      kind: "writing_preference" as const,
      summary: "称呼：您好",
      scopeSummary: "App 目标 · 通用任务",
      status: "blocked" as const,
      defaultAction: "none" as const,
      choiceOptions: [],
      reason: "mapping_selector_ambiguous",
    },
  ],
};

describe("LearningTransferSettings", () => {
  afterEach(cleanup);
  beforeEach(() => invokeMock.mockReset());

  it("does not read, preview, or apply anything until the user chooses an action", () => {
    render(<LearningTransferSettings />);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "学习配置迁移" })).toBeInTheDocument();
  });

  it("shows export counts and reports a canceled native save dialog", async () => {
    invokeMock.mockResolvedValueOnce({ status: "exported", globalWordCount: 2, pairCount: 3, preferenceCount: 1, skippedCount: 1 });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "导出学习配置" }));
    expect(await screen.findByRole("status")).toHaveTextContent("全局词条 2 项、纠正 3 项、偏好 1 项");
    expect(screen.getByRole("status")).toHaveTextContent("另有 1 项未导出");
    expect(invokeMock).toHaveBeenCalledWith("export_learning_transfer");

    invokeMock.mockResolvedValueOnce({ status: "cancelled" });
    fireEvent.click(screen.getByRole("button", { name: "导出学习配置" }));
    expect(await screen.findByRole("status")).toHaveTextContent("已取消导出");
  });

  it("keeps skipped export counts visible when no items were exported", async () => {
    invokeMock.mockResolvedValueOnce({ status: "exported", globalWordCount: 0, pairCount: 0, preferenceCount: 0, skippedCount: 2 });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "导出学习配置" }));

    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("这个文件没有可迁移项目");
    expect(status).toHaveTextContent("另有 2 项未导出");
  });

  it("describes newly created local rules only for items selected to import", async () => {
    invokeMock.mockResolvedValueOnce({
      status: "preview",
      previewId: "mapping-notice",
      expiresAtMs: 1_900_000_000_000,
      items: [
        {
          itemId: "new-ready",
          kind: "word_pair",
          summary: "新词 → 规范词",
          scopeSummary: "App 目标",
          willCreateMapping: true,
          status: "ready",
          defaultAction: "import",
          choiceOptions: [],
        },
        {
          itemId: "new-replaceable",
          kind: "word_pair",
          summary: "旧词 → 新词",
          scopeSummary: "另一个 App 目标",
          willCreateMapping: true,
          status: "conflict",
          defaultAction: "keep_local",
          choiceOptions: ["keep_local", "replace"],
          reason: "new_local_mapping",
        },
        {
          itemId: "blocked",
          kind: "writing_preference",
          summary: "称呼偏好",
          scopeSummary: "不明确目标",
          willCreateMapping: true,
          status: "blocked",
          defaultAction: "none",
          choiceOptions: [],
          reason: "new_local_mapping",
        },
        {
          itemId: "disabled-local-target",
          kind: "word_pair",
          summary: "本机停用目标的规则",
          scopeSummary: "已停用 App 目标",
          willCreateMapping: false,
          status: "blocked",
          defaultAction: "none",
          choiceOptions: [],
          reason: "mapping_disabled",
        },
      ],
    });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    await screen.findByText("本机停用目标的规则");

    const notice = "导入会为此目标新建并启用本机 App 规则；词典学习保持关闭，不会生成待确认候选。来源机权限和云端批准不会迁移。";
    expect(screen.getAllByText(notice)).toHaveLength(1);
    expect(screen.getByText("目标 App 规则已停用；导入不会重新启用它。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "替换本机项" }));
    expect(screen.getAllByText(notice)).toHaveLength(2);
  });

  it("previews each item, defaults conflicts to local, and sends explicit per-item choices on Apply", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "preview_learning_transfer") return preview;
      if (command === "apply_learning_transfer") return {
        status: "applied",
        results: [
          { itemId: "word-ready", status: "imported" },
          { itemId: "pair-conflict", status: "kept_local" },
          { itemId: "word-conflict", status: "imported" },
          { itemId: "word-already-present", status: "already_present" },
          { itemId: "preference-ready", status: "imported" },
          { itemId: "preference-conflict", status: "kept_local" },
          { itemId: "preference-blocked", status: "blocked", reason: "mapping_selector_ambiguous" },
        ],
      };
      return undefined;
    });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    expect(await screen.findByText("知呼 → 知乎")).toBeInTheDocument();
    expect(screen.getByText("预览共 7 项：2 项可导入，3 项冲突，1 项本机已有，1 项无法导入。")).toBeInTheDocument();
    expect(screen.getAllByRole("radio", { name: "保留本机（默认）" })).toHaveLength(3);
    expect(screen.getAllByRole("radio", { name: "保留本机（默认）" })[0]).toBeChecked();
    expect(screen.getByText("本机当前值：知乎企业版")).toBeInTheDocument();
    expect(screen.getByText("本机当前来源：本机纠正规则")).toBeInTheDocument();
    expect(screen.getByText("多个本机 App 规则符合此目标，无法确定目标规则。")).toBeInTheDocument();
    expect(screen.getByText("本机当前值：您好（仅替换原有称呼） · 暂停")).toBeInTheDocument();
    expect(screen.getByText("短称呼形式: 您好（仅替换原有称呼）")).toBeInTheDocument();
    expect(screen.getByText(/任务：项目更新/)).toBeInTheDocument();
    expect(screen.getByText("导入会为此目标新建并启用本机 App 规则；词典学习保持关闭，不会生成待确认候选。来源机权限和云端批准不会迁移。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "加入全局词典" }));
    fireEvent.click(screen.getByRole("button", { name: "将导入 3 项到本机" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_learning_transfer", {
      previewId: "preview-1",
      choices: [
        { itemId: "pair-conflict", action: "keep_local" },
        { itemId: "word-conflict", action: "import_global" },
        { itemId: "preference-conflict", action: "keep_local" },
      ],
    }));
    const results = screen.getByRole("region", { name: "导入结果" });
    expect(within(results).getAllByText("已导入")).toHaveLength(3);
    expect(within(results).getAllByText("已保留本机")).toHaveLength(2);
    expect(within(results).getByText("本机已存在")).toBeInTheDocument();
    expect(within(results).getByText("已阻止")).toBeInTheDocument();
    expect(within(results).getByText("多个本机 App 规则符合此目标，无法确定目标规则。")).toBeInTheDocument();
  });

  it("clears stale previews and requires the file to be reopened before another apply", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "preview_learning_transfer") return preview;
      if (command === "apply_learning_transfer") throw "learning_transfer_stale_preview:settings_changed";
      return undefined;
    });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    await screen.findByText("知呼 → 知乎");
    fireEvent.click(screen.getByRole("button", { name: "将导入 2 项到本机" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("预览已过期或本机学习配置已变化");
    expect(screen.queryByText("知呼 → 知乎")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新选择文件并预览" })).toBeEnabled();
  });

  it("shows an empty preview without an enabled apply action", async () => {
    invokeMock.mockResolvedValueOnce({ status: "preview", previewId: "empty", expiresAtMs: 1_900_000_000_000, sourceSummary: { globalWordCount: 0, pairCount: 0, preferenceCount: 0 }, items: [] });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    expect(await screen.findByText("这个文件没有可迁移项目。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /将导入/ })).not.toBeInTheDocument();
  });

  it("cancels a preview locally without applying changes", async () => {
    invokeMock.mockResolvedValueOnce(preview);
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    await screen.findByText("知呼 → 知乎");
    fireEvent.click(screen.getByRole("button", { name: "取消预览" }));

    expect(await screen.findByRole("status")).toHaveTextContent("已取消预览；本机数据未变更");
    expect(screen.queryByText("知呼 → 知乎")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.some(([command]) => command === "apply_learning_transfer")).toBe(false);
  });

  it("limits large previews to 100 visible items and pages through the rest", async () => {
    const items = Array.from({ length: 101 }, (_, index) => ({
      itemId: `word-${index}`,
      kind: "global_word" as const,
      summary: `term-${index}`,
      scopeSummary: "全局个人词条",
      status: "ready" as const,
      defaultAction: "import" as const,
      choiceOptions: [] as const,
    }));
    invokeMock.mockResolvedValueOnce({ status: "preview", previewId: "large", expiresAtMs: 1_900_000_000_000, sourceSummary: { globalWordCount: 101, pairCount: 0, preferenceCount: 0 }, items });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    expect(await screen.findByText("term-0")).toBeInTheDocument();
    expect(screen.queryByText("term-100")).not.toBeInTheDocument();
    expect(screen.getByText("第 1–100 项，共 101 项")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "将导入 101 项到本机" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "下一页" }));
    expect(await screen.findByText("term-100")).toBeInTheDocument();
    expect(screen.queryByText("term-0")).not.toBeInTheDocument();
  });

  it("shows a partial result alert and localizes Settings save failures", async () => {
    const oneReadyItem = {
      status: "preview" as const,
      previewId: "partial-preview",
      expiresAtMs: 1_900_000_000_000,
      items: [{
        itemId: "word-failed",
        kind: "global_word" as const,
        summary: "OpenAI",
        scopeSummary: "Global dictionary",
        status: "ready" as const,
        defaultAction: "import" as const,
        choiceOptions: [],
        details: { word: "OpenAI" },
      }],
    };
    invokeMock.mockImplementation(async (command) => {
      if (command === "preview_learning_transfer") return oneReadyItem;
      if (command === "apply_learning_transfer") return {
        status: "partial",
        results: [{ itemId: "word-failed", status: "failed", reason: "learning_transfer_settings_not_saved" }],
      };
      return undefined;
    });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    await screen.findByText("OpenAI");
    fireEvent.click(screen.getByRole("button", { name: "将导入 1 项到本机" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("导入部分完成；请检查逐项状态和原因");
    expect(screen.getByText("个人词条或 App 偏好未能保存。")).toBeInTheDocument();
    expect(screen.getByText("导入失败")).toBeInTheDocument();
  });

  it("explains when a local App rule was created but its correction was not saved", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "preview_learning_transfer") return preview;
      if (command === "apply_learning_transfer") return {
        status: "partial",
        results: [{ itemId: "preference-ready", status: "failed", reason: "learning_transfer_mapping_created_pair_failed" }],
      };
      return undefined;
    });
    render(<LearningTransferSettings />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件并预览" }));
    await screen.findByText("知呼 → 知乎");
    fireEvent.click(screen.getByRole("button", { name: "将导入 2 项到本机" }));

    expect(await screen.findByText("已新建本机 App 规则，但对应纠正规则未能保存。")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("导入部分完成；请检查逐项状态和原因");
  });
});
