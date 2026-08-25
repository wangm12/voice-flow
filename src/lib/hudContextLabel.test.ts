import { describe, expect, it } from "vitest";
import { formatHudContextLabel } from "./hudContextLabel";

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
