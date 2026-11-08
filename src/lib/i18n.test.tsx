import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useState } from "react";
import { I18nProvider, useI18n } from "./i18n";

afterEach(() => {
  cleanup();
});

function Probe() {
  const { language, setLanguage, t } = useI18n();
  return (
    <div>
      <span>{language}</span>
      <p>{t("主题")}</p>
      <button type="button" onClick={() => setLanguage("en")}>English</button>
    </div>
  );
}

describe("i18n", () => {
  it("starts in Chinese and switches UI copy immediately", () => {
    render(
      <I18nProvider initialLanguage="zh">
        <Probe />
      </I18nProvider>,
    );

    expect(screen.getByText("zh")).toBeInTheDocument();
    expect(screen.getByText("主题")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "English" }));

    expect(screen.getByText("en")).toBeInTheDocument();
    expect(screen.getByText("Theme")).toBeInTheDocument();
  });

  it("supports a persisted-language initial value", () => {
    render(
      <I18nProvider initialLanguage="en">
        <Probe />
      </I18nProvider>,
    );

    expect(screen.getByText("en")).toBeInTheDocument();
    expect(screen.getByText("Theme")).toBeInTheDocument();
  });
});


describe("I18nProvider language updates", () => {
  it("updates translations from a changed language prop without resetting child state", () => {
    function StatefulProbe() {
      const { t } = useI18n();
      const [count, setCount] = useState(0);
      return <div><p>{t("主题")}</p><span>{count}</span><button onClick={() => setCount((value) => value + 1)}>Increment</button></div>;
    }
    const view = render(<I18nProvider initialLanguage="zh"><StatefulProbe /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "Increment" }));
    expect(screen.getByText("1")).toBeInTheDocument();
    view.rerender(<I18nProvider initialLanguage="en"><StatefulProbe /></I18nProvider>);
    expect(screen.getByText("Theme")).toBeInTheDocument();
    expect(screen.getByText("1")).toBeInTheDocument();
  });
});
