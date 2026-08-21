import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushAnimationFrames } from "../test/flushRaf";
import { ConfirmDialog } from "./ConfirmDialog";

afterEach(() => {
  cleanup();
});

describe("ConfirmDialog", () => {
  it("does not render while closed", () => {
    render(
      <ConfirmDialog
        open={false}
        title="删除本机密钥"
        description="删除后需要重新配置。"
        confirmLabel="删除"
        cancelLabel="取消"
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("confirms from the dialog and dismisses with Escape", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        open
        title="清空全部数据"
        description="这会删除全部历史，且无法撤销。"
        confirmLabel="确定继续"
        cancelLabel="取消"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByRole("dialog", { name: "清空全部数据" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "确定继续" }));
    expect(onConfirm).toHaveBeenCalledOnce();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("restores focus to the opener after the dialog closes", async () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>删除词条</button>
          <ConfirmDialog
            open={open}
            title="删除词条"
            description="确定删除吗？"
            confirmLabel="删除"
            cancelLabel="取消"
            onConfirm={() => setOpen(false)}
            onCancel={() => setOpen(false)}
          />
        </>
      );
    }

    render(<Harness />);
    const opener = screen.getByRole("button", { name: "删除词条" });
    opener.focus();
    fireEvent.click(opener);
    await flushAnimationFrames(1);
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await flushAnimationFrames(1);
    expect(opener).toHaveFocus();
  });

  it("wraps Tab focus between the dialog actions and from the dialog container", () => {
    render(
      <ConfirmDialog
        open
        title="删除词条"
        description="确定删除吗？"
        confirmLabel="删除"
        cancelLabel="取消"
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );

    const dialog = screen.getByRole("dialog");
    const cancel = screen.getByRole("button", { name: "取消" });
    const confirm = screen.getByRole("button", { name: "删除" });
    confirm.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(confirm).toHaveFocus();
    dialog.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(cancel).toHaveFocus();
  });

  it("cancels from the presentation backdrop without exposing it to assistive tech", () => {
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        open
        title="删除词条"
        description="确定删除吗？"
        confirmLabel="删除"
        cancelLabel="取消"
        onConfirm={() => undefined}
        onCancel={onCancel}
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
      <ConfirmDialog
        open
        title="删除词条"
        description="确定删除吗？"
        confirmLabel="删除"
        cancelLabel="取消"
        onConfirm={() => undefined}
        onCancel={onCancel}
      />,
    );

    fireEvent.click(screen.getByRole("dialog"));
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("marks the background layout inert so it cannot be focused", async () => {
    render(
      <main>
        <div>
          <button type="button">背景按钮</button>
        </div>
        <ConfirmDialog
          open
          title="删除词条"
          description="确定删除吗？"
          confirmLabel="删除"
          cancelLabel="取消"
          onConfirm={() => undefined}
          onCancel={() => undefined}
        />
      </main>,
    );

    await flushAnimationFrames(1);
    const layout = document.querySelector("main")?.firstElementChild;
    expect(layout).toBeInstanceOf(HTMLElement);
    expect((layout as HTMLElement).inert).toBe(true);
    expect(screen.getByRole("button", { name: "背景按钮" })).not.toHaveFocus();
  });
});
