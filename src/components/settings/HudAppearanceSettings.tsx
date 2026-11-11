import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { EyeOff, LoaderCircle, Mic2, Moon, Sun } from "lucide-react";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { useI18n } from "../../lib/i18n";
import type { HudAppearance, SaveSettings, Settings } from "../../types/settings";
import "./HudAppearanceSettings.css";

type HudPreviewState = "recording" | "processing" | "hidden";
type HudPreviewBackdrop = "light" | "dark";
type HudFallbackReason = "unsupported_os" | "reduce_transparency" | "native_failure";

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
  const [previewState, setPreviewState] = useState<HudPreviewState>("recording");
  const [previewBackdrop, setPreviewBackdrop] = useState<HudPreviewBackdrop>("light");
  const [runtimeStatus, setRuntimeStatus] = useState<HudAppearanceStatus | null>(null);
  const [runtimeStatusLoading, setRuntimeStatusLoading] = useState(true);
  const [runtimeStatusUnavailable, setRuntimeStatusUnavailable] = useState(false);
  const [runtimeStatusUpdatesUnavailable, setRuntimeStatusUpdatesUnavailable] = useState(false);
  const [statusRetryRevision, setStatusRetryRevision] = useState(0);
  const appearance = settings.hud_appearance ?? "classic";
  const statusRevision = useRef(0);
  const previousAppearance = useRef(appearance);
  const previousSaving = useRef(saving);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    const publish = (status: HudAppearanceStatus) => {
      if (!active) return;
      statusRevision.current += 1;
      setRuntimeStatus(status);
      setRuntimeStatusUnavailable(false);
      setRuntimeStatusLoading(false);
    };

    const refresh = async () => {
      const revision = ++statusRevision.current;
      setRuntimeStatusLoading(true);
      try {
        const result = parseHudAppearanceStatus(await invoke<unknown>("get_hud_appearance_status"));
        if (active && revision === statusRevision.current) {
          setRuntimeStatus(result);
          setRuntimeStatusUnavailable(false);
          setRuntimeStatusLoading(false);
        }
      } catch {
        if (active && revision === statusRevision.current) {
          setRuntimeStatus(null);
          setRuntimeStatusUnavailable(true);
          setRuntimeStatusLoading(false);
        }
      }
    };

    void listen<unknown>("hud-appearance://status", (event) => {
      if (!active) return;
      try {
        publish(parseHudAppearanceStatus(event.payload));
      } catch {
        statusRevision.current += 1;
        setRuntimeStatus(null);
        setRuntimeStatusUnavailable(true);
        setRuntimeStatusLoading(false);
      }
    }).then((release) => {
      if (active) {
        unlisten = release;
        setRuntimeStatusUpdatesUnavailable(false);
      }
      else release();
    }).catch(() => {
      if (active) setRuntimeStatusUpdatesUnavailable(true);
    });

    const onFocus = () => { void refresh(); };
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibilityChange);
    void refresh();

    return () => {
      active = false;
      unlisten?.();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [statusRetryRevision]);

  useEffect(() => {
    const appearanceChanged = previousAppearance.current !== appearance;
    const saveFinished = previousSaving.current && !saving;
    previousAppearance.current = appearance;
    previousSaving.current = saving;
    if ((appearanceChanged && !saving) || saveFinished) {
      setStatusRetryRevision((revision) => revision + 1);
    }
  }, [appearance, saving]);

  const runtimeMatchesSaved = runtimeStatus?.requested === appearance;
  const runtimeIsFallback = runtimeStatus !== null && !runtimeStatus.pending
    && (runtimeStatus.fallback_reason !== null || runtimeStatus.effective !== runtimeStatus.requested);
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

      <SettingsGroup title={t("HUD 外观")} description={t("此偏好会应用到原生听写 HUD。") }>
        <SettingsRow title={t("外观偏好")} description={t("运行时可能因系统能力回退；请查看下方原生状态。") }>
          <div className="vf-hud-preview-options" role="group" aria-label={t("HUD 外观")}>
            {(["classic", "liquid_glass"] as const).map((value) => (
              <button
                key={value}
                type="button"
                className="vf-hud-preview-option"
                aria-pressed={appearance === value}
                onClick={() => {
                  if (appearance !== value) save({ hud_appearance: value }, { immediate: true });
                }}
              >
                {appearanceLabel(value, t)}
              </button>
            ))}
          </div>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title={t("浏览器模拟预览")}
        description={t("这是设置页中的界面模拟，不操作原生窗口，也不访问麦克风或剪贴板。")}
      >
        <SettingsRow title={t("模拟背景")} description={t("切换内置浅色和深色样例背景。") }>
          <div className="vf-hud-preview-options" role="group" aria-label={t("模拟背景")}>
            <button
              type="button"
              className="vf-hud-preview-option"
              aria-pressed={previewBackdrop === "light"}
              onClick={() => setPreviewBackdrop("light")}
            >
              <Sun size={14} aria-hidden="true" />
              {t("浅色")}
            </button>
            <button
              type="button"
              className="vf-hud-preview-option"
              aria-pressed={previewBackdrop === "dark"}
              onClick={() => setPreviewBackdrop("dark")}
            >
              <Moon size={14} aria-hidden="true" />
              {t("深色")}
            </button>
          </div>
        </SettingsRow>
        <SettingsRow title={t("模拟状态")} description={t("状态仅用于此预览。") }>
          <div className="vf-hud-preview-options" role="group" aria-label={t("模拟状态")}>
            {(["recording", "processing", "hidden"] as const).map((state) => (
              <button
                key={state}
                type="button"
                className="vf-hud-preview-option"
                aria-pressed={previewState === state}
                onClick={() => setPreviewState(state)}
              >
                {state === "recording" && <Mic2 size={14} aria-hidden="true" />}
                {state === "processing" && <LoaderCircle size={14} aria-hidden="true" />}
                {state === "hidden" && <EyeOff size={14} aria-hidden="true" />}
                {previewLabels[state]}
              </button>
            ))}
          </div>
        </SettingsRow>
        <SettingsRow className="vf-hud-preview-row" title={t("HUD 预览")} description={`${t("模拟外观：")} ${appearanceLabel(appearance, t)}`}>
          <div className="vf-hud-preview" data-backdrop={previewBackdrop} data-appearance={appearance}>
            <div className="vf-hud-preview-document" aria-hidden="true">
              <div className="vf-hud-preview-document-bar"><span /><span /><span /></div>
              <div className="vf-hud-preview-document-body">
                <span className="vf-hud-preview-line vf-hud-preview-line--title" />
                <span className="vf-hud-preview-line vf-hud-preview-line--wide" />
                <span className="vf-hud-preview-line vf-hud-preview-line--medium" />
                <span className="vf-hud-preview-line vf-hud-preview-line--wide" />
                <span className="vf-hud-preview-line vf-hud-preview-line--short" />
              </div>
            </div>
            {previewState === "hidden" ? (
              <div className="vf-hud-preview-hidden" role="status" aria-live="polite">
                <EyeOff size={15} aria-hidden="true" />
                {t("HUD 已隐藏（模拟）")}
              </div>
            ) : (
              <div className={`vf-hud-preview-pill vf-hud-preview-pill--${appearance}`} role="status" aria-live="polite">
                <span className={`vf-hud-preview-indicator vf-hud-preview-indicator--${previewState}`} aria-hidden="true">
                  {previewState === "recording" ? <Mic2 size={15} /> : <LoaderCircle size={15} />}
                </span>
                <span>{previewLabels[previewState]}</span>
                <span className="vf-hud-preview-separator" aria-hidden="true" />
                <span className="vf-hud-preview-brand">VoiceFlow</span>
              </div>
            )}
          </div>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title={t("原生运行状态")} description={t("以下状态由原生 HUD 查询提供，与浏览器模拟预览分开。") }>
        <SettingsRow title={t("HUD 实际效果")} description={t("显示原生已保存偏好、当前生效外观和系统回退原因。") }>
          <div className="vf-hud-runtime-status" aria-live="polite">
            {runtimeStatusLoading && !runtimeStatus && (
              <SettingsStatus label={t("正在读取原生状态…")} tone="neutral" />
            )}
            {runtimeStatusUnavailable && (
              <div className="vf-hud-runtime-error">
                <SettingsStatus label={t("无法读取原生 HUD 状态。")} tone="warning" />
                <button type="button" className="vf-hud-retry" onClick={() => setStatusRetryRevision((revision) => revision + 1)}>
                  {t("重试")}
                </button>
              </div>
            )}
            {runtimeStatus && (
              <>
                {runtimeStatusUpdatesUnavailable && <SettingsStatus label={t("无法接收原生 HUD 状态更新；重新打开此页可刷新。") } tone="warning" />}
                {saving && <SettingsStatus label={t("设置正在保存…")} tone="neutral" />}
                {!saving && runtimeMatchesSaved && runtimeStatus.pending && (
                  <SettingsStatus label={t("原生 HUD 正在应用所选外观…")} tone="neutral" />
                )}
                {!saving && !runtimeMatchesSaved && <SettingsStatus label={t("原生状态仍是上次保存的偏好。") } tone="neutral" />}
                <SettingsStatus
                  label={`${t("已保存偏好：")} ${appearanceLabel(runtimeStatus.requested, t)}`}
                  tone="neutral"
                />
                <SettingsStatus
                  label={`${t("当前生效：")} ${appearanceLabel(runtimeStatus.effective, t)}`}
                  tone={runtimeStatus.pending ? "neutral" : runtimeIsFallback ? "warning" : "success"}
                />
                {runtimeStatus.fallback_reason && (
                  <p className="vf-hud-runtime-reason">{t(fallbackReasonTranslation[runtimeStatus.fallback_reason])}</p>
                )}
              </>
            )}
          </div>
        </SettingsRow>
      </SettingsGroup>
    </SettingsShell>
  );
}
