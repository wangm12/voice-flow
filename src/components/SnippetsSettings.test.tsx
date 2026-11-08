import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

it("edits the original snippet without changing its identity or enabled state", async () => {
  const onChange = vi.fn().mockResolvedValue(undefined);
  render(<SnippetsSettings snippets={[{ id: "original", trigger: "旧短语", expansion: "旧内容", enabled: false }]} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: "编辑片段 旧短语" }));
  fireEvent.change(screen.getByRole("textbox", { name: "触发短语" }), { target: { value: "新短语" } });
  fireEvent.change(screen.getByRole("textbox", { name: "展开内容" }), { target: { value: "更新内容" } });
  fireEvent.click(screen.getByRole("button", { name: "保存片段" }));
  expect(onChange).toHaveBeenCalledExactlyOnceWith([{ id: "original", trigger: "新短语", expansion: "更新内容", enabled: false }]);
  await waitFor(() => expect(screen.getByRole("textbox", { name: "触发短语" })).toHaveValue(""));
});

it("retains input and supports retry after a failed native save", async () => {
  const onChange = vi.fn().mockRejectedValueOnce(new Error("disk unavailable")).mockResolvedValue(undefined);
  render(<SnippetsSettings snippets={[]} onChange={onChange} />);
  fireEvent.change(screen.getByRole("textbox", { name: "触发短语" }), { target: { value: "插入签名" } });
  fireEvent.change(screen.getByRole("textbox", { name: "展开内容" }), { target: { value: "保留这个草稿" } });
  fireEvent.click(screen.getByRole("button", { name: "添加片段" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("输入已保留");
  expect(screen.getByRole("textbox", { name: "展开内容" })).toHaveValue("保留这个草稿");
  fireEvent.click(screen.getByRole("button", { name: "添加片段" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "展开内容" })).toHaveValue(""));
  expect(onChange).toHaveBeenCalledTimes(2);
});

it("rejects a duplicate trigger while preserving both snippets", () => {
  const onChange = vi.fn();
  render(<SnippetsSettings snippets={[{ id: "a", trigger: "First", expansion: "A", enabled: true }, { id: "b", trigger: "Second", expansion: "B", enabled: true }]} onChange={onChange} />);
  fireEvent.click(screen.getByRole("button", { name: "编辑片段 First" }));
  fireEvent.change(screen.getByRole("textbox", { name: "触发短语" }), { target: { value: " second " } });
  fireEvent.click(screen.getByRole("button", { name: "保存片段" }));
  expect(screen.getByRole("alert")).toHaveTextContent("已经存在");
  expect(onChange).not.toHaveBeenCalled();
});
