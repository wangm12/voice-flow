import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SnippetsSettings } from "./SnippetsSettings";

afterEach(() => {
  cleanup();
});

describe("SnippetsSettings", () => {
  it("renders the compact form and keeps saved snippets editable", () => {
    const onChange = vi.fn();
    render(
      <SnippetsSettings
        snippets={[{ id: "snippet.email", trigger: "插入我的邮箱", expansion: "mingjie@example.com", enabled: true }]}
        onChange={onChange}
      />,
    );

    expect(screen.getByRole("textbox", { name: "触发短语" })).toBeInTheDocument();
    expect(screen.getByText("mingjie@example.com")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("switch", { name: "启用 插入我的邮箱" }));

    expect(onChange).toHaveBeenCalledWith([
      { id: "snippet.email", trigger: "插入我的邮箱", expansion: "mingjie@example.com", enabled: false },
    ]);
  });
});
