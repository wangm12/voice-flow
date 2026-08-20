import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
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

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("restores focus to the opener after the dialog closes", () => {
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
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(opener).toHaveFocus();
  });
});
