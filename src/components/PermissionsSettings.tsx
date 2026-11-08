import { invoke } from "@tauri-apps/api/core";
import { Check, Mic, Monitor, RefreshCw, TextCursorInput } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useI18n } from "../lib/i18n";
import { secondaryButtonClass } from "../lib/theme";
import { SettingsAlert, SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "./SettingsLayout";
import { usePermissionGuide } from "./Onboarding/usePermissionGuide";

export type Permissions = { microphone: boolean; microphone_status?: string; accessibility: boolean; screen_recording?: boolean };

export function PermissionsSettings({ permissions, checking = false, checkError, onRefresh }: { permissions: Permissions | null; checking?: boolean; checkError?: string | null; onRefresh: () => Promise<void> }) {
  const { t } = useI18n();
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const unknownStatus = t(checking || pending === "refresh" ? "检测中…" : "无法检测，请重试");
  const [guidePurpose, setGuidePurpose] = useState<"accessibility" | "screen" | null>(null);
  const guideAllowed = guidePurpose === "accessibility" ? permissions?.accessibility === true : guidePurpose === "screen" ? permissions?.screen_recording === true : false;
  const guide = usePermissionGuide(guidePurpose !== null && !guideAllowed, guidePurpose ?? "accessibility");
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
    setGuidePurpose("accessibility"); guide.reopen();
    await onRefresh();
  };
  const requestScreenRecording = async () => {
    setGuidePurpose("screen"); guide.reopen();
    await onRefresh();
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("系统权限")} description={t("麦克风用于录音，辅助功能用于自动粘贴。屏幕录制用于你启用的屏幕相关操作。")} actions={<button type="button" disabled={pending !== null} aria-busy={pending === "refresh"} onClick={() => void run("refresh", onRefresh)} className={`${secondaryButtonClass} min-w-28`}><RefreshCw size={16} aria-hidden="true" />{t(pending === "refresh" ? "检测中…" : "重新检测")}</button>} />
      {error && <div className="mt-4"><SettingsAlert>{error}</SettingsAlert></div>}
      {checkError && <div className="mt-4"><SettingsAlert onRetry={() => void run("refresh", onRefresh)} retryLabel={t("重新检测")}>{t(checkError)}</SettingsAlert></div>}
      <SettingsGroup title={t("权限")}>
        <PermissionSettingRow
          icon={<Mic size={18} strokeWidth={1.5} aria-hidden="true" />}
          title={t("麦克风")}
          description={t("录音和语音识别需要此权限。在系统提示或麦克风设置中允许 VoiceFlow。")}
          status={permissions?.microphone ? t("已允许") : permissions ? microphonePermissionStatus(permissions.microphone_status, t) : unknownStatus}
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
          status={permissions ? (permissions.accessibility ? t("已开启") : t("未开启，可稍后设置")) : unknownStatus}
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
          status={permissions ? (permissions.screen_recording ? t("已允许") : t("未开启，可稍后设置")) : unknownStatus}
          ok={permissions?.screen_recording ?? false}
          actionLabel={t("打开屏幕录制设置")}
          disabled={pending !== null}
          busy={pending === "screen"}
          onAction={() => void run("screen", requestScreenRecording)}
        />
      </SettingsGroup>
      {guidePurpose && <aside className="mt-4 rounded-lg border border-border p-4" aria-label={t("系统权限操作提示")}>
        <h2 className="text-sm font-medium">{t(guidePurpose === "screen" ? "仅为你主动启用的屏幕操作授权" : "在辅助功能中允许 VoiceFlow")}</h2>
        <p className="mt-2 text-xs leading-5 text-secondary">{t(guidePurpose === "screen" ? "在屏幕录制列表中找到 VoiceFlow 并开启开关；这不会授予所有 App 的上下文权限，普通听写不截图。" : "在系统设置 → 隐私与安全性 → 辅助功能中找到 VoiceFlow，开启右侧开关。")}</p>
        <p className="mt-2 text-xs leading-5 text-secondary">{t("找不到条目时，点击列表下方的添加按钮，在应用程序中选择 VoiceFlow。开发版本请选当前运行的 App。")}</p>
        {guide.status?.bundle_path && <p className="mt-2 break-all text-xs text-secondary">{t("当前运行的 App")}：{guide.status.bundle_path}</p>}
        {guide.status?.window_open && <p className="mt-2 text-xs text-secondary">{t(guide.status.drag_available ? "可移动原生提示窗已打开；可把其中的 App 文件卡片拖入系统列表，或使用添加按钮。拖拽完成不代表已授权。" : "原生提示窗已打开；当前运行环境无法提供 App 文件拖拽，请使用系统添加按钮或 Finder 路径。")}</p>}
        <p role="status" className="mt-2 text-xs text-secondary">{t(guideAllowed ? "本机权限检查已通过" : "等待本机权限检查；打开设置不代表已授权。")}</p>
        <div className="mt-3 flex flex-wrap gap-2"><button type="button" className={secondaryButtonClass} onClick={() => guide.reopen()}>{t("重新打开权限提示窗")}</button><button type="button" className={secondaryButtonClass} onClick={() => setGuidePurpose(null)}>{t("关闭提示")}</button></div>
      </aside>}
      {guide.failed && <div className="mt-4"><SettingsAlert>{t("权限提示窗释放或打开状态无法确认，请重试。")}</SettingsAlert></div>}
      <p className="mt-4 max-w-2xl text-xs leading-5 text-tertiary">{t("辅助功能权限用于自动粘贴，以及你另行启用的文字读取和操作。上下文读取与纠错学习仍需各自授权；未开启自动粘贴时，结果会复制到剪贴板。")} </p>
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
