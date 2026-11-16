import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import {
  WritingPreferenceReview,
  type WritingPreferenceCandidate,
  type WritingPreferenceDecisionItem,
  type WritingPreferenceDecisionOutcome,
} from "./WritingPreferenceReview";

afterEach(cleanup);

const candidates: WritingPreferenceCandidate[] = [
  {
    candidate_id: "candidate-project-list",
    candidate_generation: 2,
    mapping_id: "mapping-cursor",
    mapping_revision: 8,
    app_label: "Cursor",
    task_id: "project_update",
    dimension: "list_structure",
    value: "bullets",
    has_text_sample: false,
    display_scope: "App: Cursor · 输入框: 文档编辑器",
    distinct_origins: 3,
    distinct_natural_days: 2,
    distinct_sessions: 2,
    qualified_at_ms: 1_800_000_000_000,
  },
  {
    candidate_id: "candidate-chat-greeting",
    candidate_generation: 4,
    mapping_id: "mapping-slack",
    mapping_revision: 11,
    app_label: "Slack",
    task_id: "work_chat",
    dimension: "greeting_tone",
    value: "respectful_greeting",
    has_text_sample: false,
    display_scope: "App: Slack · 输入框: 聊天输入框",
    distinct_origins: 4,
    distinct_natural_days: 3,
    distinct_sessions: 3,
    qualified_at_ms: 1_800_000_001_000,
  },
];

function openReview() {
  fireEvent.click(screen.getByRole("button", { name: /待复核的写作偏好/ }));
}

describe("WritingPreferenceReview", () => {
  it("loads only after the user opens the review and displays structured candidate details", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates);
    const onDecideCandidates = vi.fn();
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);

    expect(onLoadCandidates).not.toHaveBeenCalled();
    openReview();

    expect(await screen.findByText("Cursor")).toBeInTheDocument();
    expect(screen.getByText("项目更新")).toBeInTheDocument();
    expect(screen.getByText("列表结构")).toBeInTheDocument();
    expect(screen.getByText("符合来源证据时使用要点")).toBeInTheDocument();
    expect(screen.getAllByText("不包含原文样例")).toHaveLength(2);
    expect(screen.getByText(/列表仅含 App、任务、维度、预测值和聚合计数/)).toBeInTheDocument();
    expect(screen.getByText(/3 笔独立来源 · 2 个自然日 · 2 个会话/)).toBeInTheDocument();
    expect(onLoadCandidates).toHaveBeenCalledTimes(1);
    expect(onDecideCandidates).not.toHaveBeenCalled();
  });

  it("approves and dismisses selected candidates with revision checks and shows every item outcome", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates);
    const onDecideCandidates = vi.fn(async (_items: WritingPreferenceDecisionItem[], _selected: WritingPreferenceCandidate[]) => [] as WritingPreferenceDecisionOutcome[]);
    onDecideCandidates
      .mockResolvedValueOnce([
        { candidate_id: "candidate-project-list", mapping_id: "mapping-cursor", decision: "approve", status: "approved", new_mapping_revision: 9 },
        { candidate_id: "candidate-chat-greeting", mapping_id: "mapping-slack", decision: "approve", status: "preference_conflict", new_mapping_revision: null },
      ])
      .mockResolvedValueOnce([
        { candidate_id: "candidate-chat-greeting", mapping_id: "mapping-slack", decision: "dismiss", status: "dismissed", new_mapping_revision: null },
      ]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    await screen.findByText("Cursor");

    fireEvent.click(screen.getByRole("button", { name: /全选待复核偏好/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    await waitFor(() => expect(onDecideCandidates.mock.calls[0]?.[0]).toEqual([
      { candidate_id: "candidate-project-list", candidate_generation: 2, expected_mapping_revision: 8, decision: "approve" },
      { candidate_id: "candidate-chat-greeting", candidate_generation: 4, expected_mapping_revision: 11, decision: "approve" },
    ]));
    expect(onDecideCandidates.mock.calls[0]?.[1]).toEqual(candidates);
    const projectRow = screen.getByRole("group", { name: /Cursor.*项目更新/ });
    const chatRow = screen.getByRole("group", { name: /Slack.*工作聊天/ });
    expect(within(projectRow).getByText("已批准为显式偏好")).toBeInTheDocument();
    expect(within(projectRow).getByRole("checkbox")).toBeDisabled();
    expect(within(chatRow).getByText("现有偏好与候选冲突；请检查 App 规则")).toBeInTheDocument();
    expect(within(chatRow).getByText("刷新候选后即可确认此项的当前状态。")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("处理结果：批准 1 项，忽略 0 项，需复核 1 项。");
    expect(within(chatRow).getByRole("checkbox")).not.toBeChecked();

    fireEvent.click(within(chatRow).getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /忽略所选候选/ }));
    await waitFor(() => expect(onDecideCandidates.mock.calls[1]?.[0]).toEqual([
      { candidate_id: "candidate-chat-greeting", candidate_generation: 4, expected_mapping_revision: 11, decision: "dismiss" },
    ]));
    expect(within(chatRow).getByText("已忽略此候选")).toBeInTheDocument();
    expect(within(chatRow).getByRole("checkbox")).toBeDisabled();
  });

  it("clears selected candidates and old outcomes when the user refreshes", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates);
    const onDecideCandidates = vi.fn().mockResolvedValue([
      { candidate_id: "candidate-project-list", mapping_id: "mapping-cursor", decision: "approve", status: "approved", new_mapping_revision: 9 },
    ] satisfies WritingPreferenceDecisionOutcome[]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    await screen.findByText("Cursor");
    const firstCheckbox = screen.getByRole("checkbox", { name: /Cursor.*项目更新/ });
    fireEvent.click(firstCheckbox);
    expect(firstCheckbox).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "刷新候选列表" }));

    await waitFor(() => expect(onLoadCandidates).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ })).not.toBeChecked();
    expect(screen.queryByText("已批准为显式偏好")).not.toBeInTheDocument();
  });

  it("reports an unconfirmed batch error and requires a refreshed list before another decision", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates);
    const onDecideCandidates = vi.fn().mockRejectedValue(new Error("IPC failed"));
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    await screen.findByText("Cursor");
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("批量操作结果未确认；请先刷新候选，再决定是否重试。");
    expect(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /全选待复核偏好/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /忽略所选候选/ })).toBeDisabled();
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ }));
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "刷新候选列表" }));
    await waitFor(() => expect(onLoadCandidates).toHaveBeenCalledTimes(2));
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);
  });

  it("does not refresh mappings after the review route unmounts", async () => {
    let resolveDecision: ((value: WritingPreferenceDecisionOutcome[]) => void) | undefined;
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates.slice(0, 1));
    const onDecideCandidates = vi.fn(() => new Promise<WritingPreferenceDecisionOutcome[]>((resolve) => { resolveDecision = resolve; }));
    const onRefreshApprovedMappings = vi.fn().mockResolvedValue(undefined);
    const view = render(<WritingPreferenceReview
      onLoadCandidates={onLoadCandidates}
      onDecideCandidates={onDecideCandidates}
      onRefreshApprovedMappings={onRefreshApprovedMappings}
    />);
    openReview();
    await screen.findByText("Cursor");
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));
    await waitFor(() => expect(resolveDecision).toBeDefined());
    view.unmount();

    resolveDecision?.([{
      candidate_id: "candidate-project-list",
      mapping_id: "mapping-cursor",
      decision: "approve",
      status: "approved",
      new_mapping_revision: 9,
    }]);
    await Promise.resolve();
    expect(onRefreshApprovedMappings).not.toHaveBeenCalled();
  });

  it("blocks a second review until an approved mapping refresh retry succeeds", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue(candidates);
    const onDecideCandidates = vi.fn()
      .mockResolvedValueOnce([{
        candidate_id: "candidate-project-list",
        mapping_id: "mapping-cursor",
        decision: "approve",
        status: "approved",
        new_mapping_revision: 9,
      } satisfies WritingPreferenceDecisionOutcome])
      .mockResolvedValueOnce([{
        candidate_id: "candidate-chat-greeting",
        mapping_id: "mapping-slack",
        decision: "dismiss",
        status: "dismissed",
        new_mapping_revision: null,
      } satisfies WritingPreferenceDecisionOutcome]);
    const onRefreshApprovedMappings = vi.fn()
      .mockRejectedValueOnce(new Error("mapping read failed"))
      .mockResolvedValueOnce(undefined);
    render(<WritingPreferenceReview
      onLoadCandidates={onLoadCandidates}
      onDecideCandidates={onDecideCandidates}
      onRefreshApprovedMappings={onRefreshApprovedMappings}
    />);
    openReview();
    await screen.findByText("Cursor");
    fireEvent.click(screen.getByRole("checkbox", { name: /Cursor.*项目更新/ }));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    const refreshAlert = await screen.findByRole("alert");
    expect(refreshAlert).toHaveTextContent("偏好已批准，但 App 规则未能同步");
    const remaining = screen.getByRole("checkbox", { name: /Slack.*工作聊天/ });
    expect(remaining).toBeDisabled();
    expect(screen.getByRole("button", { name: /忽略所选候选/ })).toBeDisabled();
    fireEvent.click(remaining);
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);

    fireEvent.click(within(refreshAlert).getByRole("button", { name: "重试同步 App 偏好" }));
    await waitFor(() => expect(onRefreshApprovedMappings).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(screen.getByRole("checkbox", { name: /Slack.*工作聊天/ })).not.toBeDisabled();
    const selectionToggle = screen.getByRole("button", { name: /全选待复核偏好|取消全选/ });
    if (selectionToggle.textContent?.includes("取消全选")) fireEvent.click(selectionToggle);
    fireEvent.click(screen.getByRole("button", { name: /全选待复核偏好/ }));
    expect(screen.getByRole("button", { name: /忽略所选候选/ })).not.toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /忽略所选候选/ }));
    await waitFor(() => expect(onDecideCandidates).toHaveBeenCalledTimes(2));
  });

  it("includes stable mapping scope in checkbox names when App labels and tasks match", async () => {
    const sameAppAndTaskCandidates: WritingPreferenceCandidate[] = [
      {
        ...candidates[0],
        candidate_id: "candidate-chat-scope",
        app_label: "Shared Editor",
        task_id: "work_chat",
        mapping_id: "mapping-shared-chat",
        display_scope: "App: Shared Editor · 输入框: 聊天输入框",
      },
      {
        ...candidates[0],
        candidate_id: "candidate-document-scope",
        app_label: "Shared Editor",
        task_id: "work_chat",
        mapping_id: "mapping-shared-document",
        display_scope: "App: Shared Editor · 输入框: 文档编辑器",
      },
    ];
    render(<WritingPreferenceReview
      onLoadCandidates={vi.fn().mockResolvedValue(sameAppAndTaskCandidates)}
      onDecideCandidates={vi.fn()}
    />);
    openReview();

    expect(await screen.findByRole("checkbox", { name: /Shared Editor.*工作聊天.*聊天输入框/ })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Shared Editor.*工作聊天.*文档编辑器/ })).toBeInTheDocument();
  });

  it("shows a retry action when the candidate list could not load", async () => {
    const onLoadCandidates = vi.fn()
      .mockRejectedValueOnce(new Error("storage unavailable"))
      .mockResolvedValueOnce([]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={vi.fn()} />);
    openReview();

    expect(await screen.findByRole("alert")).toHaveTextContent("候选列表读取失败；现有设置未更改。请重试。");
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(onLoadCandidates).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("目前没有待复核的写作偏好。")).toBeInTheDocument();
    expect(screen.getByText(/列表为空也可能是全局学习词条、本机改动观察或此 App 规则学习未开启/)).toBeInTheDocument();
  });

  it("accepts nullable mapping IDs and surfaces backend write failures as per-item outcomes", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue([candidates[0]]);
    const onDecideCandidates = vi.fn().mockResolvedValue([{
      candidate_id: "candidate-project-list",
      mapping_id: null,
      decision: "approve",
      status: "settings_save_failed",
      new_mapping_revision: null,
    } satisfies WritingPreferenceDecisionOutcome]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    const row = await screen.findByRole("group", { name: /Cursor.*项目更新/ });
    fireEvent.click(await within(row).findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    expect(await within(row).findByText("偏好未能写入 App 规则，请刷新候选")).toBeInTheDocument();
    expect(within(row).getByRole("checkbox")).toBeDisabled();
  });

  it("requires refreshing candidates after a History source recheck failure", async () => {
    const onLoadCandidates = vi.fn().mockResolvedValue([candidates[0]]);
    const onDecideCandidates = vi.fn().mockResolvedValue([{
      candidate_id: candidates[0].candidate_id,
      mapping_id: candidates[0].mapping_id,
      decision: "approve",
      status: "source_recheck_failed",
      new_mapping_revision: null,
    } satisfies WritingPreferenceDecisionOutcome]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    const row = await screen.findByRole("group", { name: /Cursor.*项目更新/ });
    fireEvent.click(within(row).getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: /批准所选偏好/ }));

    expect(await within(row).findByText("History 来源未能重新验证；请刷新候选列表后再决定是否重试")).toBeInTheDocument();
    expect(within(row).getByText("刷新候选后即可确认此项的当前状态。")).toBeInTheDocument();
    expect(within(row).getByRole("checkbox")).toBeDisabled();
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "刷新候选列表" }));
    await waitFor(() => expect(onLoadCandidates).toHaveBeenCalledTimes(2));
    const refreshedRow = await screen.findByRole("group", { name: /Cursor.*项目更新/ });
    expect(within(refreshedRow).queryByText("History 来源未能重新验证；请刷新候选列表后再决定是否重试")).not.toBeInTheDocument();
    expect(within(refreshedRow).getByRole("checkbox")).not.toBeDisabled();
    expect(onDecideCandidates).toHaveBeenCalledTimes(1);
  });

  it.each([
    {
      status: "mapping_unavailable" as const,
      decision: "dismiss" as const,
      message: "App 规则已停用或删除；请刷新候选列表后再复核",
    },
    {
      status: "permission_revoked" as const,
      decision: "approve" as const,
      message: "本机观察授权已撤销；请刷新候选列表后再复核",
    },
  ])("requires refresh after $status regardless of the attempted decision", async ({ status, decision, message }) => {
    const onLoadCandidates = vi.fn().mockResolvedValue([candidates[0]]);
    const onDecideCandidates = vi.fn().mockResolvedValue([{
      candidate_id: candidates[0].candidate_id,
      mapping_id: candidates[0].mapping_id,
      decision,
      status,
      new_mapping_revision: null,
    } satisfies WritingPreferenceDecisionOutcome]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    const row = await screen.findByRole("group", { name: /Cursor.*项目更新/ });
    fireEvent.click(within(row).getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: decision === "approve" ? /批准所选偏好/ : /忽略所选候选/ }));

    expect(await within(row).findByText(message)).toBeInTheDocument();
    expect(within(row).getByText("刷新候选后即可确认此项的当前状态。")).toBeInTheDocument();
    expect(within(row).getByRole("checkbox")).toBeDisabled();
    expect(onDecideCandidates.mock.calls[0]?.[0]).toHaveLength(1);

    fireEvent.click(screen.getByRole("button", { name: "刷新候选列表" }));
    await waitFor(() => expect(onLoadCandidates).toHaveBeenCalledTimes(2));
    expect(within(await screen.findByRole("group", { name: /Cursor.*项目更新/ })).getByRole("checkbox")).not.toBeDisabled();
  });

  it("caps each selection at 64 candidates and lets the user process the remaining batch", async () => {
    const manyCandidates = Array.from({ length: 65 }, (_, index) => ({
      ...candidates[0],
      candidate_id: `candidate-${index}`,
      mapping_id: `mapping-${index}`,
      app_label: `Batch App ${index}`,
    }));
    const byCandidateId = new Map(manyCandidates.map((candidate) => [candidate.candidate_id, candidate]));
    const onLoadCandidates = vi.fn().mockResolvedValue(manyCandidates);
    const onDecideCandidates = vi.fn(async (items: WritingPreferenceDecisionItem[]) => items.map((item) => ({
      candidate_id: item.candidate_id,
      mapping_id: byCandidateId.get(item.candidate_id)?.mapping_id ?? null,
      decision: item.decision,
      status: item.decision === "approve" ? "approved" as const : "dismissed" as const,
      new_mapping_revision: item.decision === "approve" ? 9 : null,
    })));
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={onDecideCandidates} />);
    openReview();
    await screen.findByText("Batch App 0");

    fireEvent.click(screen.getByRole("button", { name: /全选待复核偏好/ }));

    expect(screen.getByRole("status")).toHaveTextContent("每批最多复核 64 项；请先处理当前选择，再继续选择其余候选。");
    expect(screen.getByRole("button", { name: /忽略所选候选 \(64\)/ })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: /忽略所选候选 \(64\)/ }));
    await waitFor(() => expect(onDecideCandidates.mock.calls[0]?.[0]).toHaveLength(64));

    const finalRow = await screen.findByRole("group", { name: /Batch App 64.*项目更新/ });
    const finalCheckbox = within(finalRow).getByRole("checkbox");
    expect(finalCheckbox).not.toBeDisabled();
    fireEvent.click(finalCheckbox);
    fireEvent.click(screen.getByRole("button", { name: /忽略所选候选 \(1\)/ }));
    await waitFor(() => expect(onDecideCandidates).toHaveBeenCalledTimes(2));
    expect(onDecideCandidates.mock.calls[1]?.[0]).toEqual([{
      candidate_id: "candidate-64",
      candidate_generation: 2,
      expected_mapping_revision: 8,
      decision: "dismiss",
    }]);
  });

  it("blocks a candidate when its stable mapping cannot be joined to a live scope", async () => {
    const missingScope = { ...candidates[0], display_scope: null };
    const onLoadCandidates = vi.fn().mockResolvedValue([missingScope]);
    render(<WritingPreferenceReview onLoadCandidates={onLoadCandidates} onDecideCandidates={vi.fn()} />);
    openReview();

    const row = await screen.findByRole("group", { name: /Cursor.*项目更新/ });
    expect(within(row).getByText("无法识别当前规则范围")).toBeInTheDocument();
    expect(within(row).getByRole("alert")).toHaveTextContent("无法读取此候选对应的当前规则范围，请刷新候选列表。");
    expect(within(row).getByRole("checkbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: /全选待复核偏好/ })).toBeDisabled();
  });
});
