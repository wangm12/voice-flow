import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PermissionsStep } from "./PermissionsStep";

afterEach(cleanup);

const props = {
  onRefresh: vi.fn(),
  onRequestMicrophone: vi.fn(),
  onEnableAccessibility: vi.fn(),
  onRetryGuide: vi.fn(),
  guideOpen: false,
  nativeGuide: null,
  guideFailed: false,
  onCloseGuide: vi.fn(),
  onOpenAccessibilitySettings: vi.fn(),
  inputDevice: "",
  configurationKey: "test-session",
  onInputDeviceChange: vi.fn(async () => undefined),
  onMicrophoneStatus: vi.fn(),
};

describe("onboarding permission readback states", () => {
  it("keeps initial checks pending and reports failed readback as unknown", () => {
    const { rerender } = render(<PermissionsStep {...props} permissions={null} settingsError={null} />);
    expect(screen.getAllByText("检测中…")).toHaveLength(2);

    rerender(<PermissionsStep {...props} permissions={null} settingsError="Could not read permissions" />);
    expect(screen.getAllByText("无法检测，请重试")).toHaveLength(2);
    expect(screen.getByRole("alert")).toHaveTextContent("Could not read permissions");
    expect(screen.queryByText("未开启，可稍后设置")).not.toBeInTheDocument();
  });
});
