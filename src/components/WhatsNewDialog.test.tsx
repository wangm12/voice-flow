import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { I18nProvider } from "../lib/i18n";
import { WhatsNewDialog } from "./WhatsNewDialog";

afterEach(cleanup);

describe("WhatsNewDialog", () => {
  it("formats bundled notes and translates their content in the English interface", () => {
    render(<I18nProvider initialLanguage="en"><WhatsNewDialog payload={{ version: "0.1.0", notes: "# VoiceFlow 0.1.0\n\n- 新安装默认短按切换、按住说话，已保存按法保留。\n- 可选登录后台启动、CLI 控制和更新说明预览。" }} onDismiss={() => undefined} /></I18nProvider>);
    expect(screen.getByText("0.1.0")).toBeInTheDocument();
    expect(screen.queryByText("# VoiceFlow 0.1.0")).not.toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    expect(screen.getByText(/New installs support tap/)).toBeInTheDocument();
    expect(screen.queryByText(/新安装默认/)).not.toBeInTheDocument();
  });

  it("keeps untrusted markup as text", () => {
    const view = render(<WhatsNewDialog payload={{ version: "0.1.0", notes: '- <img src="x" onerror="alert(1)">' }} onDismiss={() => undefined} />);
    expect(screen.getByRole("listitem")).toHaveTextContent('<img src="x" onerror="alert(1)">');
    expect(view.baseElement.querySelector("img")).toBeNull();
  });
});
