import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushAnimationFrames } from "../test/flushRaf";
import { SelectedPreviewDialog } from "./SelectedPreviewDialog";

afterEach(() => {
  cleanup();
});

const preview = {
  selected_text: "原文",
  transcript: "翻译成英文",
  final_text: "Original text",
};

describe("SelectedPreviewDialog", () => {
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
    const replace = screen.getByRole("button", { name: "替换原文" });
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
});
