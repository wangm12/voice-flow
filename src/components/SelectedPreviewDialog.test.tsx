import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushAnimationFrames } from "../test/flushRaf";
import { isSelectedActionPreview, SelectedPreviewDialog, type SelectedActionPreview } from "./SelectedPreviewDialog";

afterEach(() => {
  cleanup();
});

const preview: SelectedActionPreview = {
  transaction_id: "tx-translation-1",
  action_sequence: 1,
  kind: "selected",
  operation: "translate",
  target_kind: "selection",
  target_label: "current_field",
  source_text: "原文",
  instruction: "翻译成英文",
  delivery_mode: "replace_or_copy",
  delivery_notice: "copy_if_target_changed",
  selected_text: "原文",
  transcript: "翻译成英文",
  final_text: "Original text",
  replace_allowed: true,
};

describe("SelectedPreviewDialog", () => {
  it("shows the editable result before a collapsed source without nested text scrollers", () => {
    render(<SelectedPreviewDialog preview={{ ...preview, source_text: "很长的来源文本" }} draft={preview.final_text} onDraftChange={() => undefined} onCancel={() => undefined} onCopy={() => undefined} onConfirm={() => undefined} />);
    const result = screen.getByRole("textbox", { name: "VoiceFlow 生成结果" });
    const source = screen.getByText("很长的来源文本");
    expect(result.compareDocumentPosition(source) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(source.closest("details")).not.toHaveAttribute("open");
    expect(source).not.toBeVisible();
    fireEvent.click(screen.getByText("查看来源和指令"));
    expect(source).toBeVisible();
    expect(source).not.toHaveClass("overflow-y-auto");
  });
  it("requires a positive action sequence and a contract-safe target token", () => {
    expect(isSelectedActionPreview(preview)).toBe(true);
    expect(isSelectedActionPreview({ ...preview, action_sequence: 0 })).toBe(false);
    expect(isSelectedActionPreview({ ...preview, target_label: "Private app title" })).toBe(false);
  });

  it("dismisses with Escape and restores opener focus", async () => {
    const onCancel = vi.fn();

    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>打开预览</button>
          {open && (
            <SelectedPreviewDialog
              preview={preview}
              draft={preview.final_text}
              onDraftChange={() => undefined}
              onCancel={() => {
                onCancel();
                setOpen(false);
              }}
              onCopy={() => undefined}
              onConfirm={() => undefined}
            />
          )}
        </>
      );
    }

    render(<Harness />);
    const opener = screen.getByRole("button", { name: "打开预览" });
    opener.focus();
    fireEvent.click(opener);
    await flushAnimationFrames(1);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
    await flushAnimationFrames(1);
    expect(opener).toHaveFocus();
  });

  it("keeps textarea focus while draft updates and restores opener after close", async () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      const [draft, setDraft] = useState("Original text");
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>打开预览</button>
          {open && (
            <SelectedPreviewDialog
              preview={preview}
              draft={draft}
              onDraftChange={setDraft}
              onCancel={() => void setOpen(false)}
              onCopy={() => undefined}
              onConfirm={() => undefined}
            />
          )}
        </>
      );
    }

    render(<Harness />);
    const opener = screen.getByRole("button", { name: "打开预览" });
    opener.focus();
    fireEvent.click(opener);
    await flushAnimationFrames(1);
    const textarea = screen.getByRole("textbox", { name: "VoiceFlow 生成结果" });
    textarea.focus();
    fireEvent.change(textarea, { target: { value: "Edited once" } });
    expect(textarea).toHaveFocus();
    fireEvent.change(textarea, { target: { value: "Edited twice" } });
    expect(textarea).toHaveFocus();
    expect(screen.getByRole("button", { name: "关闭" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await flushAnimationFrames(1);
    expect(opener).toHaveFocus();
  });

  it("wraps Tab focus from the dialog container and last control", () => {
    render(
      <SelectedPreviewDialog
        preview={preview}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    const dialog = screen.getByRole("dialog");
    const replace = screen.getByRole("button", { name: "确认" });
    dialog.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(dialog.contains(document.activeElement)).toBe(true);
    replace.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(document.activeElement).not.toBe(replace);
    expect(dialog.contains(document.activeElement)).toBe(true);
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(replace).toHaveFocus();
  });

  it("cancels from the presentation backdrop without exposing it to assistive tech", () => {
    const onCancel = vi.fn();
    render(
      <SelectedPreviewDialog
        preview={preview}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={onCancel}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    const backdrop = document.body.querySelector('[role="presentation"]');
    expect(backdrop).not.toBeNull();
    expect(backdrop?.tagName).toBe("DIV");
    fireEvent.click(backdrop!);
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("does not cancel when clicking inside the dialog panel", () => {
    const onCancel = vi.fn();
    render(
      <SelectedPreviewDialog
        preview={preview}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={onCancel}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    fireEvent.click(screen.getByRole("dialog"));
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("shows look-at-screen actions and drops the thumbnail on cancel", async () => {
    function Harness() {
      const [preview, setPreview] = useState<SelectedActionPreview | null>({
        kind: "screen",
        transaction_id: "tx-screen-1",
        action_sequence: 2,
        operation: "screen_assist",
        target_kind: "screen",
        target_label: "captured_screen",
        source_text: "",
        instruction: "把标题改短",
        delivery_mode: "replace_or_copy",
        delivery_notice: "copy_if_target_changed",
        selected_text: "",
        transcript: "把标题改短",
        final_text: "周五开会",
        thumbnail: "data:image/png;base64,aaa",
        replace_allowed: true,
      });
      if (!preview) return null;
      return (
        <SelectedPreviewDialog
          preview={preview}
          draft={preview.final_text}
          onDraftChange={() => undefined}
          onCancel={() => setPreview(null)}
          onCopy={() => undefined}
          onConfirm={() => undefined}
        />
      );
    }

    render(<Harness />);
    expect(screen.getByRole("dialog", { name: "看屏幕预览" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "窗口截图" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "只复制" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("img", { name: "窗口截图" })).not.toBeInTheDocument();
  });

  it("marks the settings chrome inert while the preview is open", async () => {
    render(
      <main>
        <div>
          <button type="button">设置侧栏</button>
        </div>
        <SelectedPreviewDialog
          preview={preview}
          draft={preview.final_text}
          onDraftChange={() => undefined}
          onCancel={() => undefined}
          onCopy={() => undefined}
          onConfirm={() => undefined}
        />
      </main>,
    );

    await flushAnimationFrames(1);
    const layout = document.querySelector("main")?.firstElementChild;
    expect(layout).toBeInstanceOf(HTMLElement);
    expect((layout as HTMLElement).inert).toBe(true);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("shows fixed operation and target labels without rendering backend target labels", () => {
    render(
      <SelectedPreviewDialog
        preview={preview}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.getByText("翻译")).toBeInTheDocument();
    expect(screen.getByText("选中文本")).toBeInTheDocument();
    expect(screen.getByText("原文")).toBeInTheDocument();
    expect(screen.getByText("翻译成英文")).toBeInTheDocument();
    expect(screen.getByText("如果确认前目标或来源发生变化，VoiceFlow 会只复制结果，不会覆盖新内容。")).toBeInTheDocument();
    expect(screen.queryByText("当前文本框")).not.toBeInTheDocument();
  });

  it("makes clipboard-only confirmation explicit and keeps the copy-only target message", () => {
    const clipboardPreview: SelectedActionPreview = {
      ...preview,
      operation: "draft_reply",
      target_kind: "empty_composer",
      delivery_mode: "replace_or_copy",
      delivery_notice: "clipboard_only",
      replace_allowed: false,
      source_text: "Authorized nearby context",
    };
    render(
      <SelectedPreviewDialog
        preview={clipboardPreview}
        draft={clipboardPreview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.getByText("起草回复")).toBeInTheDocument();
    expect(screen.getByText("空白回复输入框")).toBeInTheDocument();
    expect(screen.getByText("此目标目前只支持复制；确认后不会插入或发送。")).toBeInTheDocument();
    expect(screen.getByText("回复只会作为草稿写入当前输入框或复制，不会发送。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认并复制" })).toBeEnabled();
  });

  it("prevents duplicate copy or confirm while busy, while leaving cancellation available", () => {
    render(
      <SelectedPreviewDialog
        preview={preview}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
        busy
      />,
    );

    expect(screen.getByRole("textbox", { name: "VoiceFlow 生成结果" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "只复制" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "确认" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "只复制" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "确认" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("status")).toHaveTextContent("处理中…");
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();
  });

  it.each([
    ["rewrite", "改写"],
    ["shorten", "精简"],
    ["translate", "翻译"],
    ["organize", "结构整理"],
    ["draft_reply", "起草回复"],
    ["modify_exact", "按指令修改值或术语"],
    ["screen_assist", "看屏幕"],
  ] as const)("uses the fixed label for %s", (operation, label) => {
    render(
      <SelectedPreviewDialog
        preview={{ ...preview, operation }}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.queryByText(preview.target_label)).not.toBeInTheDocument();
  });

  it.each([
    ["selection", "选中文本"],
    ["field_text", "当前文本框"],
    ["empty_composer", "空白回复输入框"],
    ["screen", "已捕获的屏幕"],
  ] as const)("uses the fixed label for target %s", (target_kind, label) => {
    render(
      <SelectedPreviewDialog
        preview={{ ...preview, target_kind }}
        draft={preview.final_text}
        onDraftChange={() => undefined}
        onCancel={() => undefined}
        onCopy={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.queryByText(preview.target_label)).not.toBeInTheDocument();
  });
});
