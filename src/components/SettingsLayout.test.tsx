import { useState } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsDisclosure } from "./SettingsLayout";

afterEach(cleanup);

describe("SettingsDisclosure", () => {
  it("preserves the same mounted input and its draft without submitting", () => {
    const submit = vi.fn();
    function Draft() {
      const [value, setValue] = useState("");
      return <form onSubmit={submit}><input aria-label="草稿" value={value} onChange={(event) => setValue(event.target.value)} /></form>;
    }
    render(<SettingsDisclosure title="高级选项"><Draft /></SettingsDisclosure>);
    const trigger = screen.getByRole("button", { name: "高级选项" });
    fireEvent.click(trigger);
    const input = screen.getByRole("textbox", { name: "草稿" });
    fireEvent.change(input, { target: { value: "未保存的文字" } });
    input.focus();
    fireEvent.click(trigger);
    expect(input).not.toBeVisible();
    expect(trigger).toHaveFocus();
    expect(submit).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    expect(screen.getByRole("textbox", { name: "草稿" })).toBe(input);
    expect(input).toHaveValue("未保存的文字");
  });

  it("keeps an input's existing blur commit when focus returns to the header", () => {
    const commit = vi.fn();
    render(<SettingsDisclosure title="分段" defaultOpen><input aria-label="秒数" type="number" onBlur={commit} /></SettingsDisclosure>);
    screen.getByRole("spinbutton").focus();
    fireEvent.click(screen.getByRole("button", { name: "分段" }));
    expect(commit).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "分段" })).toHaveFocus();
  });

  it("opens when an error arrives and keeps the affected fields visible", () => {
    const { rerender } = render(<SettingsDisclosure title="连接"><input aria-label="地址" /></SettingsDisclosure>);
    expect(screen.getByLabelText("地址")).not.toBeVisible();
    act(() => rerender(<SettingsDisclosure title="连接" error="无效地址"><input aria-label="地址" /><p role="alert">无效地址</p></SettingsDisclosure>));
    expect(screen.getByRole("button", { name: "连接" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("alert")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "地址" })).toBeVisible();
  });
});
