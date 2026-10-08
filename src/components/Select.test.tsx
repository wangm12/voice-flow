import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Select } from "./Select";
import { selectOption } from "../test/selectOption";

afterEach(cleanup);

describe("Select", () => {
  it("commits a real empty default value without exposing its internal identifier", async () => {
    const change = vi.fn();
    render(<Select aria-label="Device" value="external" onValueChange={change}><option value="">System default</option><option value="external">External microphone</option></Select>);
    selectOption(screen.getByRole("combobox"), "");
    expect(change).toHaveBeenCalledExactlyOnceWith("");
    await waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
  });

  it("updates the checkmark and display when the controlled selection changes", () => {
    const { rerender } = render(<Select aria-label="Language" value="en"><option value="en">English</option><option value="zh">中文</option></Select>);
    rerender(<Select aria-label="Language" value="zh"><option value="en">English</option><option value="zh">中文</option></Select>);
    expect(screen.getByRole("combobox")).toHaveTextContent("中文");
    fireEvent.click(screen.getByRole("combobox"));
    expect(screen.getByRole("option", { name: "中文", hidden: true })).toHaveAttribute("data-state", "checked");
  });

  it("keeps the current value when Escape dismisses the menu and returns focus", async () => {
    const change = vi.fn();
    render(<Select aria-label="Language" value="en" onValueChange={change}><option value="en">English</option><option value="zh">中文</option></Select>);
    const trigger = screen.getByRole("combobox");
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const list = await screen.findByRole("listbox");
    fireEvent.keyDown(list, { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(change).not.toHaveBeenCalled();
  });

  it("selects an enabled option with the keyboard", async () => {
    function Example() {
      const [value, setValue] = useState("en");
      return <Select aria-label="Language" value={value} onValueChange={setValue}><option value="en">English</option><option value="zh">中文</option></Select>;
    }
    render(<Example />);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
    const option = await screen.findByRole("option", { name: "中文" });
    option.focus();
    fireEvent.keyDown(option, { key: "Enter" });
    await waitFor(() => expect(screen.getByRole("combobox")).toHaveTextContent("中文"));
  });

  it("blocks disabled choices", () => {
    const change = vi.fn();
    render(<Select aria-label="Language" value="en" onValueChange={change}><option value="en">English</option><option value="zh" disabled>中文</option></Select>);
    fireEvent.click(screen.getByRole("combobox"));
    const option = screen.getByRole("option", { name: "中文", hidden: true });
    expect(option).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(option);
    expect(change).not.toHaveBeenCalled();
  });

  it("closes when its parent becomes busy without committing", async () => {
    const change = vi.fn();
    const options = <><option value="en">English</option><option value="zh">中文</option></>;
    const { rerender } = render(<Select aria-label="Language" value="en" onValueChange={change}>{options}</Select>);
    fireEvent.click(screen.getByRole("combobox"));
    rerender(<Select aria-label="Language" value="en" disabled onValueChange={change}>{options}</Select>);
    await waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
    expect(screen.getByRole("combobox")).toBeDisabled();
    expect(change).not.toHaveBeenCalled();
  });
});
