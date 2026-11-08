import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DataChoices } from "./DataChoices";
import { PermissionGuide } from "./PermissionGuide";

afterEach(cleanup);

describe("optional onboarding choices", () => {
  it("shows saved independent choices and routes to their existing settings", () => {
    const onSettings = vi.fn();
    render(<DataChoices settings={{ asr_provider: "on_device", cleanup_enabled: true, keep_success_audio: false, keep_audio_days: 7, keep_history_days: 365, context_enabled: false, dictionary_learn_enabled: true }} onSettings={onSettings} />);
    fireEvent.click(screen.getByText("隐私与数据选择"));
    expect(screen.getByText(/本机识别与云端整理是独立选择/)).toBeInTheDocument();
    expect(screen.getByText(/默认关闭。保留录音不等于批准训练/)).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "前往设置" })[4]);
    expect(onSettings).toHaveBeenCalledExactlyOnceWith("context");
  });

  it("distinguishes indefinite history from immediate audio cleanup", () => {
    render(<DataChoices settings={{ keep_history_days: 0, keep_audio_days: 0 }} onSettings={vi.fn()} />);
    fireEvent.click(screen.getByText("隐私与数据选择"));
    expect(screen.getByText(/永久保留，直到手动删除/)).toHaveTextContent("保留音频：立即清理");
    expect(screen.queryByText(/保留天数：0/)).not.toBeInTheDocument();
  });

  it("keeps missing or malformed saved values unknown", () => {
    render(<DataChoices settings={{ asr_provider: "unrecognized", cleanup_enabled: "false", keep_success_audio: null, keep_history_days: -1 }} onSettings={vi.fn()} />);
    fireEvent.click(screen.getByText("隐私与数据选择"));
    expect(screen.getAllByText("无法确认").length).toBeGreaterThanOrEqual(5);
    expect(screen.queryByText("已开启")).not.toBeInTheDocument();
    expect(screen.queryByText("已关闭")).not.toBeInTheDocument();
  });

  it("keeps permission unconfirmed after opening system settings and supports closing", () => {
    const onOpenSettings = vi.fn();
    const onClose = vi.fn();
    render(<PermissionGuide granted={false} onRefresh={vi.fn()} onOpenSettings={onOpenSettings} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "打开辅助功能设置" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(screen.getByRole("status")).toHaveTextContent("等待本机权限检查；打开设置不代表已授权。");
    fireEvent.click(screen.getByRole("button", { name: "稍后开启，先使用复制" }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("lets the user reopen when the last native receipt still says open", () => {
    const onRetryGuide = vi.fn();
    render(<PermissionGuide
      granted={false}
      nativeGuide={{ guide_id: "guide", permission: "accessibility", bundle_path: null, bundle_identifier: null, signature_status: "unknown", drag_available: false, permission_allowed: false, window_open: true, detail: null, placement: "adjacent_settings_window" }}
      onRefresh={vi.fn()}
      onOpenSettings={vi.fn()}
      onRetryGuide={onRetryGuide}
      onClose={vi.fn()}
    />);
    fireEvent.click(screen.getByRole("button", { name: "重新打开权限提示窗" }));
    expect(onRetryGuide).toHaveBeenCalledOnce();
  });
});
