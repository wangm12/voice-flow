import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
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
