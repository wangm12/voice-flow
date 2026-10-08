import { describe, expect, it } from "vitest";
import { formatHudContextLabel, formatHudContextSource, formatHudIntensityLabel, formatHudTranslationLabel } from "./hudContextLabel";

it("shows only supported translation targets with a localized action label", () => {
  expect(formatHudTranslationLabel("ja", (value) => value)).toBe("翻译 → 日本語");
  expect(formatHudTranslationLabel("en", () => "Translate")).toBe("Translate → English");
  expect(formatHudTranslationLabel("untrusted text", (value) => value)).toBeNull();
  expect(formatHudTranslationLabel(null, (value) => value)).toBeNull();
});

describe("formatHudContextSource", () => {
  it("formats only the allowlisted source kind", () => {
    expect(formatHudContextSource("ax")).toBe("辅助功能文字");
    expect(formatHudContextSource("ocr")).toBe("本机 OCR");
    expect(formatHudContextSource("cloud_vision")).toBe("云端视觉");
    expect(formatHudContextSource("none")).toBeNull();
    expect(formatHudContextSource(null)).toBeNull();
  });

  it("translates fixed source labels without accepting caller text", () => {
    expect(formatHudContextSource("ax", (value) => value === "辅助功能文字" ? "Accessibility text" : value)).toBe("Accessibility text");
  });
});

describe("formatHudContextLabel", () => {
  it("composes a known app with a Chinese style label", () => {
    expect(formatHudContextLabel("Cursor", "prompt_or_code", "Cursor · Code")).toBe("Cursor · 代码");
    expect(formatHudContextLabel("Outlook", "email", "Outlook · 邮件")).toBe("Outlook · 邮件");
    expect(formatHudContextLabel("Todoist", "calendar_task", "Todoist · Planning")).toBe(
      "Todoist · 日程",
    );
  });

  it("uses 未知应用 when the app name is missing", () => {
    expect(formatHudContextLabel(null, "general", "未知 App · 通用")).toBe("未知应用 · 通用");
    expect(formatHudContextLabel("General", "general", null)).toBe("未知应用 · 通用");
  });

  it("falls back to the precomposed label when structured parts are absent", () => {
    expect(formatHudContextLabel(undefined, undefined, "WeChat · 口语")).toBe("WeChat · 口语");
  });

  it("translates style labels through the i18n function", () => {
    const t = (source: string) => {
      if (source === "代码") return "Code";
      if (source === "未知应用") return "Unknown app";
      if (source === "通用") return "General";
      return source;
    };
    expect(formatHudContextLabel("Cursor", "prompt_or_code", null, t)).toBe("Cursor · Code");
    expect(formatHudContextLabel(null, "general", null, t)).toBe("Unknown app · General");
  });
});

describe("formatHudIntensityLabel", () => {
  it("shows app and resolved intensity only", () => {
    expect(formatHudIntensityLabel("WeChat", "heavy")).toBe("WeChat · 重");
    expect(formatHudIntensityLabel("WeChat", "light")).toBe("WeChat · 轻");
    expect(formatHudIntensityLabel("WeChat", "standard")).toBe("WeChat · 中");
    expect(formatHudIntensityLabel("WeChat", "off")).toBe("WeChat · 关");
  });

  it("uses 未知应用 when the app name is missing", () => {
    expect(formatHudIntensityLabel(null, "heavy")).toBe("未知应用 · 重");
    expect(formatHudIntensityLabel("General", "heavy")).toBe("未知应用 · 重");
  });

  it("returns null without a resolved intensity", () => {
    expect(formatHudIntensityLabel("WeChat", null)).toBeNull();
    expect(formatHudIntensityLabel("WeChat", undefined)).toBeNull();
  });

  it("translates intensity labels through the i18n function", () => {
    const t = (source: string) => {
      if (source === "重") return "Heavy";
      if (source === "未知应用") return "Unknown app";
      return source;
    };
    expect(formatHudIntensityLabel("WeChat", "heavy", t)).toBe("WeChat · Heavy");
    expect(formatHudIntensityLabel(null, "heavy", t)).toBe("Unknown app · Heavy");
  });
});
