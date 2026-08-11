import { invoke } from "@tauri-apps/api/core";
import { Check, Mic, RefreshCw, TextCursorInput } from "lucide-react";
import type React from "react";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass } from "../lib/theme";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "./SettingsLayout";

export type Permissions = { microphone: boolean; microphone_status?: string; accessibility: boolean };

export function PermissionsSettings({ permissions, onRefresh }: { permissions: Permissions | null; onRefresh: () => Promise<void> }) {
  const { t } = useI18n();
  const requestMicrophone = async () => {
    if (permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted") {
      await invoke("open_privacy_settings", { pane: "microphone" });
    } else {
      await invoke("request_microphone_permission");
    }
    await onRefresh();
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("系统权限")} description={t("麦克风和自动粘贴是两个独立的 macOS 权限。只有在查看此页面时才会实时检测状态。")} actions={<button type="button" onClick={() => void onRefresh()} className={secondaryButtonClass}><RefreshCw size={15} aria-hidden="true" />{t("重新检测")}</button>} />
      <SettingsGroup title={t("权限")}>
        <PermissionSettingRow
          icon={<Mic size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("麦克风")}
          description={t("录音和语音识别需要此权限。")}
          status={permissions?.microphone ? t("已允许") : permissions ? microphonePermissionStatus(permissions.microphone_status, t) : t("检测中…")}
          ok={permissions?.microphone ?? false}
          actionLabel={t(permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted" ? "打开设置" : "开启权限")}
          onAction={() => void requestMicrophone()}
        />
        <PermissionSettingRow
          icon={<TextCursorInput size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("自动粘贴")}
          description={t("将文字直接写入当前光标；未开启时会复制到剪贴板。")}
          status={permissions ? (permissions.accessibility ? t("已开启") : t("未开启，可稍后设置")) : t("检测中…")}
          ok={permissions?.accessibility ?? false}
          actionLabel={t("开启权限")}
          onAction={() => void invoke("open_privacy_settings", { pane: "accessibility" })}
        />
      </SettingsGroup>
      <p className="mt-4 max-w-2xl text-xs leading-5 text-tertiary">{t("自动粘贴权限只用于将结果写入当前光标；未开启时，VoiceFlow 会保留文字并复制到剪贴板。")} </p>
    </SettingsShell>
  );
}

function PermissionSettingRow({ icon, title, description, status, ok, actionLabel, onAction }: { icon: React.ReactNode; title: string; description: string; status: string; ok: boolean; actionLabel: string; onAction: () => void }) {
  return (
    <SettingsRow title={title} description={description} icon={icon}>
      <SettingsStatus label={status} tone={ok ? "success" : "warning"} />
      {ok ? <Check size={17} className="text-success" aria-label={status} /> : <button type="button" onClick={onAction} className={`${secondaryButtonClass} shrink-0`}>{actionLabel}</button>}
    </SettingsRow>
  );
}

function microphonePermissionStatus(status: string | undefined, t: (source: string) => string): string {
  if (status === "denied") return t("需要在系统设置中开启");
  if (status === "restricted") return t("系统限制，无法授权");
  if (status === "unknown") return t("无法检测，请重试");
  return t("等待授权");
}
