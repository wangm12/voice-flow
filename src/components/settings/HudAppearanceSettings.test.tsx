import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "../../lib/i18n";
import type { SaveSettings, Settings } from "../../types/settings";
import { HudAppearanceSettings } from "./HudAppearanceSettings";
import {
  TEXT_CONTRAST,
  adapt,
  contrast,
  hexRgb,
  luma,
  pickInk,
  tintFloor,
} from "./hud-glasswork/GlassworkAdapt";
import {
  cornerExponent,
  rimDisplacement,
} from "./hud-glasswork/GlassworkLens";
import { drawGlassworkScene } from "./hud-glasswork/GlassworkRenderer";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue({
    requested: "classic",
    effective: "classic",
    fallback_reason: null,
    pending: false,
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(vi.fn()),
}));

vi.mock("./HudGlassPreview", () => ({
  HudGlassPreview: ({
    backdrop,
    refract,
  }: {
    backdrop: string;
    refract: boolean;
  }) => (
    <div
      data-testid="hud-glass-preview"
      data-backdrop={backdrop}
      data-refract={String(refract)}
    />
  ),
}));

function baseSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    schema_version: 25,
    api_key_configured: true,
    api_key_hint: "gsk_…abcd",
    asr_model: "whisper-large-v3-turbo",
    cleanup_model: "openai/gpt-oss-20b",
    language: "auto",
    ui_language: "zh",
    theme: "system",
    hud_appearance: "classic",
    hud_color_variant: "colorful",
    hud_processing_color_variant: "colorful",
    hud_voice_style: "ripple",
    hud_processing_style: "flow_line",
    hud_voice_intensity: 1,
    dictionary: [],
    hotkey: "Fn",
    activation_mode: "tap",
    chunk_threshold_secs: 25,
    chunk_length_secs: 35,
    long_output_mode: "paste",
    delivery_policy: "auto",
    keep_audio_days: 7,
    keep_history_days: 365,
    onboarded: true,
    cleanup_enabled: true,
    show_tray_icon: true,
    context_enabled: false,
    browser_access_enabled: false,
    context_mappings: [],
    writing_modes: [],
    snippets: [],
    output_mode: "auto",
    translation_target_language: "en",
    input_device: "",
    ...overrides,
  };
}

describe("HudAppearanceSettings studio playground", () => {
  beforeEach(() => {
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("defaults to Classic, renders textless color pills, and updates voice & processing previews independently", async () => {
    const save = vi.fn<SaveSettings>().mockResolvedValue(undefined);
    const { rerender } = render(
      <I18nProvider initialLanguage="zh">
        <HudAppearanceSettings settings={baseSettings({ hud_appearance: undefined })} save={save} saving={false} />
      </I18nProvider>,
    );

    expect(screen.getByRole("button", { name: "Classic" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Liquid Glass" })).toHaveAttribute("aria-pressed", "false");
    expect(document.querySelector(".vf-hud-pg")).not.toBeNull();

    const resetBtn = screen.getByRole("button", { name: "恢复默认光效" });
    expect(resetBtn).toBeDisabled();

    // Voice wave colors and processing beam colors are configured independently and have no text or inner dot elements.
    const voiceColorGroup = screen.getByRole("group", { name: "声浪色彩" });
    const processingColorGroup = screen.getByRole("group", { name: "扫光色彩" });

    const voiceOcean = within(voiceColorGroup).getByRole("button", { name: "深海蔚蓝" });
    expect(voiceOcean.textContent).toBe("");
    expect(voiceOcean.childElementCount).toBe(0);

    fireEvent.click(voiceOcean);
    expect(save).toHaveBeenCalledWith({ hud_color_variant: "ocean" }, { immediate: true });
    expect(document.querySelector(".vf-hud-preview")).toHaveAttribute("data-preview-state", "recording");
    const waveBeam = document.querySelector('[data-voice-beam="mock"]');
    expect(waveBeam).toHaveAttribute("data-voice-color", "ocean");
    expect(waveBeam).toHaveAttribute("data-voice-band-core", "#e0f7ff");
    expect(waveBeam).toHaveAttribute("data-voice-band-mid", "#2f80ed");
    expect(waveBeam).toHaveAttribute("data-voice-active", "true");
    expect(within(processingColorGroup).getByRole("button", { name: "全彩流光" })).toHaveAttribute("aria-pressed", "true");

    // Switching processing color switches the preview to processing and updates BorderBeam independently of voice wave color.
    const processingForest = within(processingColorGroup).getByRole("button", { name: "森屿翠绿" });
    expect(processingForest.textContent).toBe("");
    fireEvent.click(processingForest);
    expect(save).toHaveBeenCalledWith({ hud_processing_color_variant: "forest" }, { immediate: true });
    expect(document.querySelector(".vf-hud-preview")).toHaveAttribute("data-preview-state", "processing");
    expect(document.querySelector('[data-beam-size="line"]')).toHaveAttribute("data-beam-color", "ocean");
    expect(document.querySelector<HTMLElement>(".voice-pill__processing-beam-layer")?.style.filter).toBe("hue-rotate(-95deg)");
    expect(within(voiceColorGroup).getByRole("button", { name: "深海蔚蓝" })).toHaveAttribute("aria-pressed", "true");

    // Switching processing style updates BorderBeam immediately on the processing stage.
    fireEvent.click(screen.getByRole("button", { name: "流光疾扫" }));
    expect(save).toHaveBeenCalledWith({ hud_processing_style: "brilliant_sweep" }, { immediate: true });
    const immediateBeam = document.querySelector('[data-beam-size="line"]');
    expect(immediateBeam).toHaveAttribute("data-beam-duration", "1.45");
    expect(immediateBeam).toHaveAttribute("data-beam-brightness", "1.55");
    expect(immediateBeam).toHaveAttribute("data-beam-active", "true");

    // Switching voice wave style from processing immediately switches preview back to recording wave.
    fireEvent.click(screen.getByRole("button", { name: "棱镜脉冲" }));
    expect(save).toHaveBeenCalledWith({ hud_voice_style: "prism" }, { immediate: true });
    expect(document.querySelector(".vf-hud-preview")).toHaveAttribute("data-preview-state", "recording");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-flow", "84");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "true");

    // Monochrome passes neutral silver/white bandColors so the canvas wave is never rainbow.
    fireEvent.click(within(voiceColorGroup).getByRole("button", { name: "极简单色" }));
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-band-core", "#ffffff");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-band-mid", "#a3a3a3");

    fireEvent.click(screen.getByRole("button", { name: "强烈" }));
    expect(save).toHaveBeenCalledWith({ hud_voice_intensity: 1.35 }, { immediate: true });

    const slider = screen.getByRole("slider", { name: "声浪灵敏度" });
    fireEvent.change(slider, { target: { value: "1.25" } });
    expect(save).toHaveBeenCalledWith({ hud_voice_intensity: 1.25 });

    rerender(
      <I18nProvider initialLanguage="zh">
        <HudAppearanceSettings
          settings={baseSettings({
            hud_appearance: "liquid_glass",
            hud_color_variant: "mono",
            hud_processing_color_variant: "forest",
            hud_voice_style: "prism",
            hud_processing_style: "brilliant_sweep",
            hud_voice_intensity: 1.25,
          })}
          save={save}
          saving={false}
        />
      </I18nProvider>,
    );

    const enabledResetBtn = screen.getByRole("button", { name: "恢复默认光效" });
    expect(enabledResetBtn).toBeEnabled();
    fireEvent.click(enabledResetBtn);
    expect(save).toHaveBeenCalledWith(
      {
        hud_appearance: "classic",
        hud_color_variant: "colorful",
        hud_processing_color_variant: "colorful",
        hud_voice_style: "ripple",
        hud_processing_style: "flow_line",
        hud_voice_intensity: 1,
      },
      { immediate: true },
    );

    fireEvent.click(within(voiceColorGroup).getByRole("button", { name: "深海蔚蓝" }));
    fireEvent.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(save).toHaveBeenLastCalledWith(
      {
        hud_appearance: "classic",
        hud_color_variant: "ocean",
        hud_processing_color_variant: "colorful",
        hud_voice_style: "ripple",
        hud_processing_style: "flow_line",
        hud_voice_intensity: 1,
      },
      { immediate: true },
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "已应用到 HUD" })).toBeInTheDocument());
  });

  it("toggles simulated Demo Voice wave on and off without accessing microphone APIs", () => {
    const getUserMedia = vi.fn();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia },
    });
    const save = vi.fn<SaveSettings>().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLanguage="zh">
        <HudAppearanceSettings settings={baseSettings()} save={save} saving={false} />
      </I18nProvider>,
    );

    const demoToggle = screen.getByRole("button", { name: "模拟声浪" });
    expect(demoToggle).toHaveAttribute("aria-pressed", "true");
    expect(demoToggle).toHaveAttribute("title", "模拟声浪 · 未使用麦克风");
    expect(document.querySelector(".vf-hud-stage-topbar .vf-hud-preview-label")).toBeNull();
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "true");
    expect(Number(document.querySelector('[data-voice-beam="mock"]')?.getAttribute("data-voice-level"))).toBeGreaterThan(0);

    fireEvent.click(demoToggle);
    expect(demoToggle).toHaveAttribute("aria-pressed", "false");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "false");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-level", "0");

    fireEvent.click(demoToggle);
    expect(demoToggle).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector('[data-voice-beam="mock"]')).toHaveAttribute("data-voice-active", "true");
    expect(getUserMedia).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "隐藏" }));
    expect(document.querySelector(".vf-hud-preview-empty")).toHaveTextContent("HUD 已隐藏（模拟）");
  });

  it("keeps preview backdrop solid without scene controls and switches light/dark backdrop", () => {
    const save = vi.fn<SaveSettings>().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLanguage="zh">
        <HudAppearanceSettings settings={baseSettings()} save={save} saving={false} />
      </I18nProvider>,
    );

    const preview = screen.getByTestId("hud-glass-preview");
    expect(preview).toHaveAttribute("data-backdrop", "dark");
    expect(preview).toHaveAttribute("data-refract", "false");
    expect(screen.queryByRole("group", { name: "预览场景" })).toBeNull();

    const backdropGroup = screen.getByRole("group", { name: "模拟背景" });
    const lightBtn = within(backdropGroup).getByRole("button", { name: "浅色" });
    const darkBtn = within(backdropGroup).getByRole("button", { name: "深色" });
    expect(darkBtn).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(lightBtn);
    expect(lightBtn).toHaveAttribute("aria-pressed", "true");
    expect(preview).toHaveAttribute("data-backdrop", "light");

    fireEvent.click(screen.getByRole("button", { name: "Liquid Glass" }));
    expect(preview).toHaveAttribute("data-refract", "true");
    expect(preview).toHaveAttribute("data-backdrop", "light");
  });

  it("renders flat light/dark and transitional solid frames in drawGlassworkScene without fake pill overlays", () => {
    for (const backdrop of ["dark", "light"] as const) {
      const fillStyles: unknown[] = [];
      let currentFillStyle: unknown = "";
      const ctx = {
        setTransform: vi.fn(),
        clearRect: vi.fn(),
        save: vi.fn(),
        beginPath: vi.fn(),
        roundRect: vi.fn(),
        clip: vi.fn(),
        restore: vi.fn(),
        fillRect: vi.fn(() => {
          fillStyles.push(currentFillStyle);
        }),
        createLinearGradient: vi.fn(),
        arc: vi.fn(),
        strokeRect: vi.fn(),
        get fillStyle() {
          return currentFillStyle;
        },
        set fillStyle(value: unknown) {
          currentFillStyle = value;
        },
      };
      const canvas = document.createElement("canvas");
      vi.spyOn(canvas, "getContext").mockReturnValue(ctx as unknown as CanvasRenderingContext2D);

      expect(drawGlassworkScene(canvas, 500, 440, 2, backdrop)).toBe(true);
      expect(canvas.width).toBe(1000);
      expect(canvas.height).toBe(880);
      expect(ctx.fillRect).toHaveBeenCalledTimes(1);
      expect(ctx.fillRect).toHaveBeenCalledWith(0, 0, 500, 440);
      expect(fillStyles[0]).toBe(backdrop === "dark" ? "#121212" : "#f2f3f5");
      expect(ctx.roundRect).not.toHaveBeenCalled();

      expect(drawGlassworkScene(canvas, 500, 440, 2, backdrop, 0.5)).toBe(true);
      expect(fillStyles[1]).toBe("rgb(130 131 132)");
      expect(ctx.createLinearGradient).not.toHaveBeenCalled();
      expect(ctx.arc).not.toHaveBeenCalled();
      expect(ctx.strokeRect).not.toHaveBeenCalled();
    }
  });

  it("keeps >= 4.5:1 WCAG contrast via Glasswork tintFloor and computes smooth rim displacement", () => {
    const inkRgb = { light: [245, 245, 247] as [number, number, number], dark: [29, 29, 31] as [number, number, number] };
    const configs = [
      { kind: "light" as const, ink: "dark" as const, thin: 0.06, dense: 0.22, tintHex: "#ffffff" },
      { kind: "dark" as const, ink: "light" as const, thin: 0.16, dense: 0.38, tintHex: "#1e1e22" },
    ];

    for (const cfg of configs) {
      const tintLevel = luma(hexRgb(cfg.tintHex));
      for (const luminance of [0.05, 0.25, 0.5, 0.75, 0.95]) {
        for (const texture of [0, 0.25, 0.5]) {
          const { density, saturate } = adapt({ luminance, texture, color: [160, 120, 210] }, cfg.ink);
          expect(density).toBeGreaterThanOrEqual(0);
          expect(density).toBeLessThanOrEqual(1);
          expect(saturate).toBeGreaterThanOrEqual(0.45);

          const floor = tintFloor(luminance, texture, cfg.ink, tintLevel);
          const alpha = Math.min(0.95, Math.max(floor, cfg.thin + (cfg.dense - cfg.thin) * density));
          const spread = texture / 2;
          const worstBackdrop = cfg.ink === "light"
            ? Math.min(1, luminance + spread)
            : Math.max(0, luminance - spread);
          const compositedLevel = Math.round((worstBackdrop + (tintLevel - worstBackdrop) * alpha) * 255);
          const bg: [number, number, number] = [compositedLevel, compositedLevel, compositedLevel];
          expect(contrast(inkRgb[cfg.ink], bg)).toBeGreaterThanOrEqual(TEXT_CONTRAST);
        }
      }
    }

    expect(pickInk(0.2)).toBe("light");
    expect(pickInk(0.8)).toBe("dark");
    expect(cornerExponent("squircle")).toBe(4);
    expect(cornerExponent("round")).toBe(2);
    const [dx, dy] = rimDisplacement(2, 24, 220, 48, { bezel: 14, radius: 24 });
    expect(dx).toBeGreaterThan(0);
    expect(dy).toBeCloseTo(0, 5);
    expect(rimDisplacement(110, 24, 220, 48, { bezel: 14, radius: 24 })).toEqual([0, 0]);
  });
});
