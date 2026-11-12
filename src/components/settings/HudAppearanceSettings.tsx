import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Moon, Sun } from "lucide-react";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell } from "../SettingsLayout";
import { Select } from "../Select";
import { VoicePill } from "../Island/VoicePill";
import { HudGlassPreview } from "./HudGlassPreview";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass } from "../../lib/theme";
import { voicePillWindowHeight, voicePillWindowWidth } from "../Island/voicePillTokens";
import type { HudAppearance, SaveSettings, Settings } from "../../types/settings";
import "../../island.css";
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
  const [previewSelection, setPreviewSelection] = useState<HudPreviewSelection>("auto");
  const [previewState, setPreviewState] = useState<HudPreviewState>("recording");
  const [previewBackdrop, setPreviewBackdrop] = useState<HudPreviewBackdrop>("light");
  const [runtimeStatus, setRuntimeStatus] = useState<HudAppearanceStatus | null>(null);
  const [runtimeStatusReadFailed, setRuntimeStatusReadFailed] = useState(false);
  const appearance = settings.hud_appearance ?? "classic";
  const statusRevision = useRef(0);
  const refreshStatus = useRef<() => void>(() => {});
  const previousSaving = useRef(saving);

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
      }
      else release();
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
  const previewLabels: Record<HudPreviewState, string> = {
    recording: t("录音中"),
    processing: t("处理中"),
    hidden: t("隐藏"),
  };

  return (
    <SettingsShell className="vf-hud-settings">
      <SettingsPageHeader
        title={t("HUD 配置")}
        description={t("选择 HUD 外观，并预览听写状态。")}
        saveHint={t("更改会自动保存。")}
      />

      <SettingsGroup title={t("外观")}>
        <SettingsRow title={t("外观偏好")} description={appearanceDescription}>
          <Select
            aria-label={t("HUD 外观")}
            value={appearance}
            onValueChange={(value) => {
              if (value === "classic" || value === "liquid_glass") {
                if (appearance !== value) save({ hud_appearance: value }, { immediate: true });
              }
            }}
            className="vf-hud-appearance-select"
          >
            {(["classic", "liquid_glass"] as const).map((value) => (
              <option key={value} value={value}>{appearanceLabel(value, t)}</option>
            ))}
          </Select>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title={t("预览")}
        description={t("预览使用样例数据，玻璃材质为模拟效果。")}
      >
        <SettingsRow
          className="vf-hud-preview-row"
          title={t("HUD 预览")}
          description={`${t("模拟外观：")} ${appearanceLabel(appearance, t)}`}
        >
          <div className="vf-hud-preview-content">
            <div className="vf-hud-preview-toolbar">
              <div className="vf-hud-backdrop-options" role="group" aria-label={t("模拟背景")}>
                <button
                  type="button"
                  className={`${compactButtonClass} vf-hud-choice`}
                  aria-pressed={previewBackdrop === "light"}
                  onClick={() => setPreviewBackdrop("light")}
                >
                  <Sun size={14} aria-hidden="true" />
                  {t("浅色")}
                </button>
                <button
                  type="button"
                  className={`${compactButtonClass} vf-hud-choice`}
                  aria-pressed={previewBackdrop === "dark"}
                  onClick={() => setPreviewBackdrop("dark")}
                >
                  <Moon size={14} aria-hidden="true" />
                  {t("深色")}
                </button>
              </div>
              <label className="vf-hud-state-select">
                <span>{t("模拟状态")}</span>
                <Select
                  aria-label={t("模拟状态")}
                  value={previewSelection}
                  onValueChange={(value) => {
                    if (value === "auto") {
                      setPreviewState("recording");
                      setPreviewSelection("auto");
                    } else if (value === "recording" || value === "processing" || value === "hidden") {
                      setPreviewState(value);
                      setPreviewSelection(value);
                    }
                  }}
                  className="vf-hud-preview-state-select"
                >
                  <option value="auto">{t("自动循环")}</option>
                  {(["recording", "processing", "hidden"] as const).map((state) => (
                    <option key={state} value={state}>{previewLabels[state]}</option>
                  ))}
                </Select>
              </label>
            </div>

            <div className="vf-hud-preview" data-backdrop={previewBackdrop} data-preview-state={previewState}>
              {previewState === "hidden" ? (
                <span className="vf-hud-preview-label vf-hud-preview-hidden">
                  {t("HUD 已隐藏（模拟）")}
                </span>
              ) : (
                <span className="vf-hud-preview-label">{t("模拟")}</span>
              )}
              <HudGlassPreview
                backdrop={previewBackdrop}
                refract={appearance === "liquid_glass" && (previewSelection === "auto" || previewState !== "hidden")}
              />
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
                    audioStatus={t("预设演示 · 未使用麦克风")}
                    audioDeviceName={t("演示输入设备")}
                    audioDeviceSourceLabel={t("预览")}
                    audioDeviceTitle={t("演示输入设备；未使用麦克风")}
                    waveformLevels={[]}
                    lastAudioLevelAtMs={null}
                    reduced
                  />
                </div>
              </div>
            </div>
          </div>
        </SettingsRow>
      </SettingsGroup>
    </SettingsShell>
  );
}
