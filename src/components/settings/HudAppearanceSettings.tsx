import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AudioLines, Check, Moon, Pause, RotateCcw, Sun } from "lucide-react";
import { SettingsPageHeader, SettingsShell } from "../SettingsLayout";
import { VoicePill } from "../Island/VoicePill";
import {
  DEFAULT_HUD_VISUALS,
  HUD_COLOR_VARIANTS,
  HUD_PROCESSING_STYLES,
  HUD_VOICE_STYLES,
  normalizeHudVisuals,
  sampleDemoVoiceLevel,
} from "../Island/hudOrb";
import { useReducedMotionPreference } from "../Island/springs";
import { HudGlassPreview } from "./HudGlassPreview";
import { useI18n } from "../../lib/i18n";
import { voicePillWindowHeight, voicePillWindowWidth } from "../Island/voicePillTokens";
import type { HudAppearance, SaveSettings, Settings } from "../../types/settings";
import "../../island.css";
import "./hud-glasswork/glasswork.css";
import "./HudAppearanceSettings.css";

type HudPreviewState = "recording" | "processing" | "hidden";
type HudPreviewSelection = "auto" | HudPreviewState;
type HudPreviewBackdrop = "light" | "dark";
type HudFallbackReason = "unsupported_os" | "reduce_transparency" | "native_failure";

const previewStateDurationMs: Record<HudPreviewState, number> = {
  recording: 3_000,
  processing: 2_000,
  hidden: 1_000,
};

function nextPreviewState(state: HudPreviewState): HudPreviewState {
  if (state === "recording") return "processing";
  if (state === "processing") return "hidden";
  return "recording";
}

type HudAppearanceStatus = {
  requested: HudAppearance;
  effective: HudAppearance;
  fallback_reason: HudFallbackReason | null;
  pending: boolean;
};

const fallbackReasonTranslation: Record<HudFallbackReason, string> = {
  unsupported_os: "当前 macOS 版本不支持 Liquid Glass。",
  reduce_transparency: "系统“降低透明度”设置使 Liquid Glass 暂不可用。",
  native_failure: "原生 HUD 未能应用所选外观。",
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function parseHudAppearanceStatus(value: unknown): HudAppearanceStatus {
  if (!isRecord(value)
    || (value.requested !== "classic" && value.requested !== "liquid_glass")
    || (value.effective !== "classic" && value.effective !== "liquid_glass")
    || (value.fallback_reason !== null
      && value.fallback_reason !== "unsupported_os"
      && value.fallback_reason !== "reduce_transparency"
      && value.fallback_reason !== "native_failure")
    || typeof value.pending !== "boolean") {
    throw new Error("invalid_hud_appearance_status");
  }
  return {
    requested: value.requested,
    effective: value.effective,
    fallback_reason: value.fallback_reason,
    pending: value.pending,
  };
}

function appearanceLabel(appearance: HudAppearance, t: (source: string) => string) {
  return appearance === "liquid_glass" ? t("Liquid Glass") : t("Classic");
}

const DEMO_VOICE_INITIAL_OFFSET_MS = 792;

const VOICE_INTENSITY_PRESETS = [
  { value: 0.75, label: "柔和" },
  { value: 1, label: "标准" },
  { value: 1.35, label: "强烈" },
] as const;

export function HudAppearanceSettings({
  settings,
  save,
  saving,
}: {
  settings: Settings;
  save: SaveSettings;
  saving: boolean;
}) {
  const { t } = useI18n();
  const reduced = useReducedMotionPreference();
  const persistedAppearance: HudAppearance = settings.hud_appearance === "liquid_glass" ? "liquid_glass" : "classic";
  const persistedVisuals = normalizeHudVisuals(settings);
  const [localAppearance, setLocalAppearance] = useState<HudAppearance>(persistedAppearance);
  const [localVisuals, setLocalVisuals] = useState(persistedVisuals);
  const [previewSelection, setPreviewSelection] = useState<HudPreviewSelection>("recording");
  const [previewState, setPreviewState] = useState<HudPreviewState>("recording");
  const [previewBackdrop, setPreviewBackdrop] = useState<HudPreviewBackdrop>("dark");
  const [demoVoiceActive, setDemoVoiceActive] = useState(true);
  const [demoVoiceLevel, setDemoVoiceLevel] = useState(() => sampleDemoVoiceLevel(DEMO_VOICE_INITIAL_OFFSET_MS));
  const [runtimeStatus, setRuntimeStatus] = useState<HudAppearanceStatus | null>(null);
  const [runtimeStatusReadFailed, setRuntimeStatusReadFailed] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [savedConfirm, setSavedConfirm] = useState(false);
  const [savingManual, setSavingManual] = useState(false);
  const appearance = localAppearance;
  const visuals = localVisuals;
  const demoStartRef = useRef<number | null>(null);
  const statusRevision = useRef(0);
  const refreshStatus = useRef<() => void>(() => {});
  const previousSaving = useRef(saving);

  useEffect(() => {
    setLocalAppearance(persistedAppearance);
  }, [persistedAppearance]);

  useEffect(() => {
    setLocalVisuals({
      colorVariant: persistedVisuals.colorVariant,
      processingColorVariant: persistedVisuals.processingColorVariant,
      voiceStyle: persistedVisuals.voiceStyle,
      processingStyle: persistedVisuals.processingStyle,
      voiceIntensity: persistedVisuals.voiceIntensity,
    });
  }, [
    persistedVisuals.colorVariant,
    persistedVisuals.processingColorVariant,
    persistedVisuals.voiceStyle,
    persistedVisuals.processingStyle,
    persistedVisuals.voiceIntensity,
  ]);

  const focusWavePreview = (resetPhase = true) => {
    if (resetPhase || demoStartRef.current === null || !demoVoiceActive || previewState !== "recording") {
      demoStartRef.current = performance.now() - DEMO_VOICE_INITIAL_OFFSET_MS;
      setDemoVoiceLevel(sampleDemoVoiceLevel(DEMO_VOICE_INITIAL_OFFSET_MS));
    }
    setDemoVoiceActive(true);
    setPreviewState("recording");
    setPreviewSelection("recording");
  };

  const focusProcessingPreview = () => {
    setPreviewState("processing");
    setPreviewSelection("processing");
  };

  const demoRunning = demoVoiceActive && !reduced && previewState === "recording";

  useEffect(() => {
    if (!demoRunning) return;
    if (demoStartRef.current === null) {
      demoStartRef.current = performance.now() - DEMO_VOICE_INITIAL_OFFSET_MS;
    }
    const updateDemo = () => {
      if (document.hidden) return;
      const elapsed = performance.now() - (demoStartRef.current ?? 0);
      setDemoVoiceLevel(sampleDemoVoiceLevel(elapsed));
    };
    updateDemo();
    const timer = window.setInterval(updateDemo, 50);
    const handleVisibility = () => {
      if (!document.hidden) updateDemo();
    };
    document.addEventListener("visibilitychange", handleVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", handleVisibility);
    };
  }, [demoRunning]);

  useEffect(() => {
    if (previewSelection !== "auto") return;

    let timer: number | undefined;
    const clearTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    };
    const schedule = () => {
      clearTimer();
      if (document.hidden) return;
      timer = window.setTimeout(
        () => setPreviewState(nextPreviewState(previewState)),
        previewStateDurationMs[previewState],
      );
    };
    const handleVisibilityChange = () => {
      if (document.hidden) {
        clearTimer();
      } else {
        schedule();
      }
    };

    document.addEventListener("visibilitychange", handleVisibilityChange);
    schedule();

    return () => {
      clearTimer();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [previewSelection, previewState]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    const refresh = async () => {
      const revision = ++statusRevision.current;
      setRuntimeStatusReadFailed(false);
      try {
        const result = parseHudAppearanceStatus(await invoke<unknown>("get_hud_appearance_status"));
        if (active && revision === statusRevision.current) {
          setRuntimeStatus(result);
          setRuntimeStatusReadFailed(false);
        }
      } catch {
        if (active && revision === statusRevision.current) {
          setRuntimeStatus(null);
          setRuntimeStatusReadFailed(true);
        }
      }
    };
    refreshStatus.current = () => { void refresh(); };

    void listen<unknown>("hud-appearance://status", (event) => {
      if (!active) return;
      try {
        const status = parseHudAppearanceStatus(event.payload);
        statusRevision.current += 1;
        setRuntimeStatus(status);
        setRuntimeStatusReadFailed(false);
      } catch {
        statusRevision.current += 1;
        setRuntimeStatus(null);
        setRuntimeStatusReadFailed(true);
      }
    }).then((release) => {
      if (active) {
        unlisten = release;
      } else release();
    }).catch(() => {
      // The one-shot read still provides the current status if event setup fails.
    });
    void refresh();

    return () => {
      active = false;
      unlisten?.();
      refreshStatus.current = () => {};
    };
  }, []);

  useEffect(() => {
    const saveFinished = previousSaving.current && !saving;
    previousSaving.current = saving;
    if (saveFinished) refreshStatus.current();
  }, [saving]);

  const runtimeIsFallback = runtimeStatus !== null
    && runtimeStatus.requested === appearance
    && !saving
    && !runtimeStatus.pending
    && (runtimeStatus.fallback_reason !== null || runtimeStatus.effective !== runtimeStatus.requested);
  const appearanceDescription = runtimeIsFallback && runtimeStatus
    ? t(fallbackReasonTranslation[runtimeStatus.fallback_reason ?? "native_failure"])
    : runtimeStatusReadFailed && appearance === "liquid_glass" && !saving
      ? t("无法读取原生 HUD 状态。")
      : t("此偏好会应用到原生听写 HUD。");
  const isDefaultAll = appearance === "classic"
    && visuals.colorVariant === DEFAULT_HUD_VISUALS.colorVariant
    && visuals.processingColorVariant === DEFAULT_HUD_VISUALS.processingColorVariant
    && visuals.voiceStyle === DEFAULT_HUD_VISUALS.voiceStyle
    && visuals.processingStyle === DEFAULT_HUD_VISUALS.processingStyle
    && Math.abs(visuals.voiceIntensity - DEFAULT_HUD_VISUALS.voiceIntensity) < 0.01;
  const stateOptions: ReadonlyArray<{ id: HudPreviewSelection; label: string }> = [
    { id: "recording", label: t("录音中") },
    { id: "processing", label: t("处理中") },
    { id: "hidden", label: t("隐藏") },
    { id: "auto", label: t("自动循环") },
  ];

  const applyHudConfig = () => {
    setSavingManual(true);
    void Promise.resolve(
      save(
        {
          hud_appearance: appearance,
          hud_color_variant: visuals.colorVariant,
          hud_processing_color_variant: visuals.processingColorVariant,
          hud_voice_style: visuals.voiceStyle,
          hud_processing_style: visuals.processingStyle,
          hud_voice_intensity: visuals.voiceIntensity,
        },
        { immediate: true },
      ),
    ).finally(() => {
      setSavingManual(false);
      setDirty(false);
      setSavedConfirm(true);
      refreshStatus.current();
    });
  };

  return (
    <SettingsShell className="vf-hud-settings">
      <SettingsPageHeader
        title={t("HUD 配置")}
        description={t("选择 HUD 外观，并预览听写状态。")}
        saveHint={t("选择后点击“保存并应用”，或直接按键呼出 HUD 查看效果。")}
      />

      <div className="vf-hud-pg">
        <div className="vf-hud-preview vf-hud-pg-stage" data-backdrop={previewBackdrop} data-preview-state={previewState}>
          <div className="vf-hud-stage-topbar">
            <div className="vf-hud-stage-pills" role="group" aria-label={t("模拟背景")}>
              <button
                type="button"
                className="vf-hud-stage-pill"
                aria-pressed={previewBackdrop === "light"}
                onClick={() => setPreviewBackdrop("light")}
              >
                <Sun size={12} aria-hidden="true" />
                {t("浅色")}
              </button>
              <button
                type="button"
                className="vf-hud-stage-pill"
                aria-pressed={previewBackdrop === "dark"}
                onClick={() => setPreviewBackdrop("dark")}
              >
                <Moon size={12} aria-hidden="true" />
                {t("深色")}
              </button>
            </div>
          </div>

          <HudGlassPreview
            backdrop={previewBackdrop}
            refract={appearance === "liquid_glass" && (previewSelection === "auto" || previewState !== "hidden")}
            reducedMotion={reduced}
          />

          {previewState === "hidden" && (
            <span className="vf-hud-preview-empty">{t("HUD 已隐藏（模拟）")}</span>
          )}

          <div className="vf-hud-preview-hud" data-appearance={appearance} data-hidden={previewState === "hidden" || undefined}>
            <div
              className="voice-pill-stage vf-hud-preview-stage"
              aria-hidden="true"
              inert
              style={{ width: voicePillWindowWidth, height: voicePillWindowHeight }}
            >
              <VoicePill
                state={previewState === "hidden" ? "idle" : previewState}
                hideImmediately={previewState === "hidden"}
                phase={previewState === "processing" ? "cleanup" : "idle"}
                phaseConfirmed={previewState === "processing"}
                stageElapsedMs={42_000}
                contextLabel={null}
                audioStatus={demoRunning ? t("模拟声浪 · 未使用麦克风") : t("预设演示 · 未使用麦克风")}
                audioDeviceName={t("演示输入设备")}
                audioDeviceSourceLabel={t("预览")}
                audioDeviceTitle={t("演示输入设备；未使用麦克风")}
                waveformLevels={[]}
                lastAudioLevelAtMs={null}
                previewDemoLevel={demoRunning ? demoVoiceLevel : null}
                visuals={visuals}
                beamTheme={appearance === "liquid_glass" ? previewBackdrop : "dark"}
                reduced={reduced}
              />
            </div>
          </div>

          <div className="vf-hud-stage-bottombar">
            <button
              type="button"
              className="vf-hud-stage-pill vf-hud-demo-toggle"
              aria-pressed={demoVoiceActive && !reduced}
              disabled={reduced}
              title={reduced ? t("系统已开启“降低动态效果”，动态声浪已暂停。") : t("模拟声浪 · 未使用麦克风")}
              onClick={() => {
                if (demoVoiceActive) {
                  setDemoVoiceActive(false);
                } else {
                  focusWavePreview(true);
                }
              }}
            >
              {demoVoiceActive && !reduced ? <Pause size={12} aria-hidden="true" /> : <AudioLines size={12} aria-hidden="true" />}
              {t("模拟声浪")}
            </button>
            <div className="vf-hud-stage-pills" role="group" aria-label={t("模拟状态")}>
              {stateOptions.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className="vf-hud-stage-pill"
                  aria-pressed={previewSelection === option.id}
                  onClick={() => {
                    if (option.id === "auto") {
                      setPreviewState("recording");
                      setPreviewSelection("auto");
                    } else {
                      setPreviewState(option.id);
                      setPreviewSelection(option.id);
                    }
                  }}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
        </div>

        <aside className="vf-hud-pg-controls" aria-label={t("HUD 配置")}>
          <section className="vf-hud-pg-section">
            <span className="vf-hud-pg-heading">{t("HUD 外观")}</span>
            <div className="vf-hud-pg-tabs" role="group" aria-label={t("HUD 外观")}>
              {(["classic", "liquid_glass"] as const).map((value) => (
                <button
                  key={value}
                  type="button"
                  className="vf-hud-pg-tab"
                  aria-pressed={appearance === value}
                  onClick={() => {
                    if (appearance !== value) {
                      setLocalAppearance(value);
                      setDirty(true);
                      setSavedConfirm(false);
                      save({ hud_appearance: value }, { immediate: true });
                      if (previewState === "hidden") focusWavePreview(true);
                    }
                  }}
                >
                  {appearanceLabel(value, t)}
                </button>
              ))}
            </div>
            <p className="vf-hud-pg-note">{appearanceDescription}</p>
          </section>

          <section className="vf-hud-pg-section">
            <span className="vf-hud-pg-heading">{t("语音声浪")}</span>

            <div className="vf-hud-pg-subfield">
              <span className="vf-hud-pg-label">{t("声浪色彩")}</span>
              <div className="vf-hud-pg-swatches" role="group" aria-label={t("声浪色彩")}>
                {HUD_COLOR_VARIANTS.map((variant) => (
                  <button
                    key={variant.id}
                    type="button"
                    className="vf-hud-pg-swatch"
                    data-variant={variant.id}
                    aria-label={t(variant.label)}
                    title={t(variant.label)}
                    aria-pressed={visuals.colorVariant === variant.id}
                    style={{
                      ["--vf-tag-gradient" as string]: `linear-gradient(120deg, ${variant.swatches[0]} 0%, ${variant.swatches[1]} 52%, ${variant.swatches[2]} 100%)`,
                    }}
                    onClick={() => {
                      setLocalVisuals((prev) => ({ ...prev, colorVariant: variant.id }));
                      setDirty(true);
                      setSavedConfirm(false);
                      save({ hud_color_variant: variant.id }, { immediate: true });
                      focusWavePreview(true);
                    }}
                  />
                ))}
              </div>
            </div>

            <div className="vf-hud-pg-subfield">
              <span className="vf-hud-pg-label">{t("语音声浪风格")}</span>
              <div className="vf-hud-pg-tabs" role="group" aria-label={t("语音声浪风格")}>
                {HUD_VOICE_STYLES.map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    className="vf-hud-pg-tab"
                    title={t(preset.description)}
                    aria-pressed={visuals.voiceStyle === preset.id}
                    onClick={() => {
                      setLocalVisuals((prev) => ({ ...prev, voiceStyle: preset.id }));
                      setDirty(true);
                      setSavedConfirm(false);
                      save({ hud_voice_style: preset.id }, { immediate: true });
                      focusWavePreview(true);
                    }}
                  >
                    {t(preset.label)}
                  </button>
                ))}
              </div>
            </div>

            <div className="vf-hud-pg-subfield">
              <div className="vf-hud-pg-intensity-head">
                <span className="vf-hud-pg-label">
                  {t("声浪灵敏度")}
                  <span className="vf-hud-pg-value">{Math.round(visuals.voiceIntensity * 100)}%</span>
                </span>
                <div className="vf-hud-pg-tabs vf-hud-pg-tabs--compact" role="group" aria-label={t("声浪灵敏度")}>
                  {VOICE_INTENSITY_PRESETS.map((preset) => (
                    <button
                      key={preset.value}
                      type="button"
                      className="vf-hud-pg-tab vf-hud-pg-tab--compact"
                      aria-pressed={Math.abs(visuals.voiceIntensity - preset.value) < 0.03}
                      onClick={() => {
                        setLocalVisuals((prev) => ({ ...prev, voiceIntensity: preset.value }));
                        setDirty(true);
                        setSavedConfirm(false);
                        save({ hud_voice_intensity: preset.value }, { immediate: true });
                        focusWavePreview(false);
                      }}
                    >
                      {t(preset.label)}
                    </button>
                  ))}
                </div>
              </div>
              <input
                type="range"
                min="0.5"
                max="1.5"
                step="0.05"
                className="vf-hud-intensity-slider"
                aria-label={t("声浪灵敏度")}
                value={visuals.voiceIntensity}
                onChange={(event) => {
                  const nextIntensity = Number(event.target.value);
                  setLocalVisuals((prev) => ({ ...prev, voiceIntensity: nextIntensity }));
                  setDirty(true);
                  setSavedConfirm(false);
                  save({ hud_voice_intensity: nextIntensity });
                  focusWavePreview(false);
                }}
              />
            </div>
          </section>

          <section className="vf-hud-pg-section">
            <span className="vf-hud-pg-heading">{t("处理扫光")}</span>

            <div className="vf-hud-pg-subfield">
              <span className="vf-hud-pg-label">{t("扫光色彩")}</span>
              <div className="vf-hud-pg-swatches" role="group" aria-label={t("扫光色彩")}>
                {HUD_COLOR_VARIANTS.map((variant) => (
                  <button
                    key={variant.id}
                    type="button"
                    className="vf-hud-pg-swatch"
                    data-variant={variant.id}
                    aria-label={t(variant.label)}
                    title={t(variant.label)}
                    aria-pressed={visuals.processingColorVariant === variant.id}
                    style={{
                      ["--vf-tag-gradient" as string]: `linear-gradient(120deg, ${variant.swatches[0]} 0%, ${variant.swatches[1]} 52%, ${variant.swatches[2]} 100%)`,
                    }}
                    onClick={() => {
                      setLocalVisuals((prev) => ({ ...prev, processingColorVariant: variant.id }));
                      setDirty(true);
                      setSavedConfirm(false);
                      save({ hud_processing_color_variant: variant.id }, { immediate: true });
                      focusProcessingPreview();
                    }}
                  />
                ))}
              </div>
            </div>

            <div className="vf-hud-pg-subfield">
              <span className="vf-hud-pg-label">{t("处理扫光风格")}</span>
              <div className="vf-hud-pg-tabs" role="group" aria-label={t("处理扫光风格")}>
                {HUD_PROCESSING_STYLES.map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    className="vf-hud-pg-tab"
                    title={t(preset.description)}
                    aria-pressed={visuals.processingStyle === preset.id}
                    onClick={() => {
                      setLocalVisuals((prev) => ({ ...prev, processingStyle: preset.id }));
                      setDirty(true);
                      setSavedConfirm(false);
                      save({ hud_processing_style: preset.id }, { immediate: true });
                      focusProcessingPreview();
                    }}
                  >
                    {t(preset.label)}
                  </button>
                ))}
              </div>
            </div>
          </section>

          <div className="vf-hud-pg-footer">
            <button
              type="button"
              className="vf-hud-pg-save"
              disabled={saving || savingManual}
              onClick={applyHudConfig}
            >
              {saving || savingManual
                ? t("保存中…")
                : !dirty && savedConfirm
                  ? <><Check size={13} aria-hidden="true" />{t("已应用到 HUD")}</>
                  : t("保存并应用")}
            </button>
            <button
              type="button"
              className="vf-hud-pg-reset"
              disabled={isDefaultAll}
              onClick={() => {
                setLocalAppearance("classic");
                setLocalVisuals(DEFAULT_HUD_VISUALS);
                setDirty(true);
                setSavedConfirm(false);
                save({
                  hud_appearance: "classic",
                  hud_color_variant: DEFAULT_HUD_VISUALS.colorVariant,
                  hud_processing_color_variant: DEFAULT_HUD_VISUALS.processingColorVariant,
                  hud_voice_style: DEFAULT_HUD_VISUALS.voiceStyle,
                  hud_processing_style: DEFAULT_HUD_VISUALS.processingStyle,
                  hud_voice_intensity: DEFAULT_HUD_VISUALS.voiceIntensity,
                }, { immediate: true });
                if (previewState === "hidden") focusWavePreview(true);
              }}
            >
              <RotateCcw size={12} aria-hidden="true" />
              {t("恢复默认光效")}
            </button>
          </div>
        </aside>
      </div>
    </SettingsShell>
  );
}
