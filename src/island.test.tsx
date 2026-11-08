import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { invokeMock, listenMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("./components/Island/IslandWindow", async () => {
  const React = await import("react");
  const { useI18n } = await import("./lib/i18n");
  return { IslandWindow: () => {
    const { t } = useI18n();
    const [count, setCount] = React.useState(0);
    return <div><p>{t("主题")}</p><span>{count}</span><button onClick={() => setCount((value) => value + 1)}>Increment</button></div>;
  } };
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.resetModules();
  vi.clearAllMocks();
});

describe("island language events", () => {
  it("translates the mounted HUD when the language event changes without resetting child state", async () => {
    let onLanguage: ((event: { payload: string }) => void) | undefined;
    invokeMock.mockResolvedValue({ ui_language: "zh" });
    listenMock.mockImplementation(async (_name: string, handler: (event: { payload: string }) => void) => {
      onLanguage = handler;
      return vi.fn();
    });
    document.body.innerHTML = '<div id="root"></div>';
    await import("./island");
    expect(await screen.findByText("主题")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Increment" }));
    await act(async () => { onLanguage?.({ payload: "en" }); });
    expect(await screen.findByText("Theme")).toBeInTheDocument();
    expect(screen.getByText("1")).toBeInTheDocument();
  });
});
