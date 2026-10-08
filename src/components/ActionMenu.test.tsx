import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActionMenu } from "./ActionMenu";

afterEach(cleanup);

describe("ActionMenu", () => {
  const makeItems = (select = vi.fn()) => [
    { label: "原文", onSelect: select },
    { label: "不可用", disabled: true, onSelect: select },
    { label: "删除", onSelect: select, danger: true },
  ];

  it("opens from the keyboard, skips disabled actions, and returns focus on Escape", () => {
    const select = vi.fn();
    render(<ActionMenu label="更多操作" items={makeItems(select)} />);
    const trigger = screen.getByRole("button", { name: "更多操作" });
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "原文" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "删除" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "原文" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(select).not.toHaveBeenCalled();
  });

  it("opens at the last action with ArrowUp and performs a selection once", () => {
    const select = vi.fn();
    render(<ActionMenu label="更多操作" items={makeItems(select)} />);
    const trigger = screen.getByRole("button", { name: "更多操作" });
    fireEvent.keyDown(trigger, { key: "ArrowUp" });
    const last = screen.getByRole("menuitem", { name: "删除" });
    expect(last).toHaveFocus();
    fireEvent.click(last);
    expect(select).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("closes on an outside click without performing an action", () => {
    const select = vi.fn();
    render(<><ActionMenu label="更多操作" items={makeItems(select)} /><input aria-label="下一项" /></>);
    const trigger = screen.getByRole("button", { name: "更多操作" });
    fireEvent.click(trigger);
    fireEvent.pointerDown(screen.getByRole("textbox"));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(select).not.toHaveBeenCalled();
  });
});
