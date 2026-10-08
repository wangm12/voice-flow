import { invoke } from "@tauri-apps/api/core";
import { Check, Mic, Monitor, RefreshCw, TextCursorInput } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass } from "../lib/theme";
import { SettingsAlert, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "./SettingsLayout";

export type Permissions = { microphone: boolean; microphone_status?: string; accessibility: boolean; screen_recording?: boolean };

export function PermissionsSettings({ permissions, onRefresh }: { permissions: Permissions | null; onRefresh: () => Promise<void> }) {
  const { t } = useI18n();
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async (id: string, action: () => Promise<unknown>) => {
    setPending(id);
    setError(null);
    try { await action(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setPending(null); }
  };
  const requestMicrophone = async () => {
    if (permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted") {
      await invoke("open_privacy_settings", { pane: "microphone" });
    } else {
      await invoke("request_microphone_permission");
    }
    await onRefresh();
  };
  const requestAccessibility = async () => {
    const granted = await invoke<boolean>("request_accessibility_permission");
    if (!granted) {
      await invoke("open_privacy_settings", { pane: "accessibility" });
    }
    await onRefresh();
  };
  const requestScreenRecording = async () => {
    await invoke("open_privacy_settings", { pane: "screen" });
    await onRefresh();
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("系统权限")} description={t("麦克风用于录音，辅助功能用于自动粘贴。屏幕录制用于你启用的屏幕相关操作。")} actions={<button type="button" disabled={pending !== null} aria-busy={pending === "refresh"} onClick={() => void run("refresh", onRefresh)} className={`${secondaryButtonClass} min-w-28`}><RefreshCw size={16} aria-hidden="true" />{t(pending === "refresh" ? "检测中…" : "重新检测")}</button>} />
      {error && <div className="mt-4"><SettingsAlert>{error}</SettingsAlert></div>}
      <SettingsGroup title={t("权限")}>
        <PermissionSettingRow
          icon={<Mic size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("麦克风")}
          description={t("录音和语音识别需要此权限。在系统提示或麦克风设置中允许 VoiceFlow。")}
          status={permissions?.microphone ? t("已允许") : permissions ? microphonePermissionStatus(permissions.microphone_status, t) : t("检测中…")}
          ok={permissions?.microphone ?? false}
          actionLabel={t(permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted" ? "打开麦克风设置" : "允许麦克风")}
          disabled={pending !== null}
          busy={pending === "microphone"}
          onAction={() => void run("microphone", requestMicrophone)}
        />
        <PermissionSettingRow
          icon={<TextCursorInput size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("自动粘贴")}
          description={t("在辅助功能设置中允许 VoiceFlow，将文字写入当前光标；未开启时会复制到剪贴板。")}
          status={permissions ? (permissions.accessibility ? t("已开启") : t("未开启，可稍后设置")) : t("检测中…")}
          ok={permissions?.accessibility ?? false}
          actionLabel={t("开启自动粘贴")}
          disabled={pending !== null}
          busy={pending === "accessibility"}
          onAction={() => void run("accessibility", requestAccessibility)}
        />
        <PermissionSettingRow
          icon={<Monitor size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("屏幕录制")}
          description={t("用于你启用的窗口文字识别或看屏幕操作。默认听写不会截屏。")}
          status={permissions ? (permissions.screen_recording ? t("已允许") : t("未开启，可稍后设置")) : t("检测中…")}
          ok={permissions?.screen_recording ?? false}
          actionLabel={t("打开屏幕录制设置")}
          disabled={pending !== null}
          busy={pending === "screen"}
          onAction={() => void run("screen", requestScreenRecording)}
        />
      </SettingsGroup>
      <p className="mt-4 max-w-2xl text-xs leading-5 text-tertiary">{t("自动粘贴权限只用于将结果写入当前光标；未开启时，VoiceFlow 会保留文字并复制到剪贴板。")} </p>
    </SettingsShell>
  );
}

function PermissionSettingRow({ icon, title, description, status, ok, actionLabel, onAction, disabled, busy }: { icon: ReactNode; title: string; description: string; status: string; ok: boolean; actionLabel: string; onAction: () => void; disabled: boolean; busy: boolean }) {
  const { t } = useI18n();
  return (
    <SettingsRow title={title} description={description} icon={icon}>
      <SettingsStatus label={status} tone={ok ? "success" : "warning"} />
      {ok ? <Check size={16} className="text-success-ink" aria-hidden="true" /> : <button type="button" disabled={disabled} aria-busy={busy} onClick={onAction} className={`${secondaryButtonClass} min-w-36 shrink-0`}>{busy ? t("处理中…") : actionLabel}</button>}
    </SettingsRow>
  );
}

function microphonePermissionStatus(status: string | undefined, t: (source: string) => string): string {
  if (status === "denied") return t("需要在系统设置中开启");
  if (status === "restricted") return t("系统限制，无法授权");
  if (status === "unknown") return t("无法检测，请重试");
  return t("等待授权");
}
