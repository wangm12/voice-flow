import { Check, Mic, RefreshCw, TextCursorInput } from "lucide-react";
import type React from "react";
import { compactButtonClass } from "../../lib/theme";
import { iconProps, iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

function statusLabel(kind: "microphone" | "accessibility", ok: boolean, status: string, t: (source: string) => string): string {
  if (ok) return t("已授权");
  if (kind === "microphone") {
    if (status === "denied") return t("需要在系统设置中开启");
    if (status === "restricted") return t("系统限制，无法授权");
    if (status === "checking") return t("检测中…");
    if (status === "unknown") return t("无法检测，请重试");
    return t("等待授权");
  }
  return t("未开启，可稍后设置");
}

function PermissionRow({
  icon,
  kind,
  title,
  ok,
  status,
  action,
}: {
  icon: React.ReactNode;
  kind: "microphone" | "accessibility";
  title: string;
  ok: boolean;
  status: string;
  action?: React.ReactNode;
}) {
  const { t } = useI18n();
  const label = statusLabel(kind, ok, status, t);

  return (
    <div className="flex items-center gap-3 border-t border-border py-4 first:border-t-0">
      <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg ${ok ? "bg-success/10 text-success" : "bg-elevated text-secondary"}`}>
        {icon}
      </div>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-primary">{t(title)}</p>
        <p className={`text-xs ${ok ? "text-success" : "text-tertiary"}`}>{label}</p>
      </div>
      {action}
      {ok && (
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-success/15 text-success">
          <Check {...iconPropsSm} aria-hidden="true" />
        </span>
      )}
    </div>
  );
}

export function PermissionsStep({
  permissions,
  settingsError,
  onRefresh,
  onRequestMicrophone,
  onEnableAccessibility,
}: {
  permissions: { microphone: boolean; microphone_status: string; accessibility: boolean } | null;
  settingsError: string | null;
  onRefresh: () => void;
  onRequestMicrophone: () => void;
  onEnableAccessibility: () => void;
}) {
  const { t } = useI18n();
  return (
    <div>
      <h1 className="text-2xl font-semibold tracking-tight text-primary">
        {t("先确认必要权限")}
      </h1>
      <p className="mt-1.5 text-sm text-secondary">
        {t("麦克风用于录音；开启自动粘贴权限后，文字会直接写入当前光标。未开启时，结果会复制到剪贴板。")}
      </p>

      <div className="mt-6 border-y border-border">
        <PermissionRow
          icon={<Mic {...iconProps} aria-hidden="true" />}
          kind="microphone"
          title="麦克风"
          ok={permissions?.microphone ?? false}
          status={permissions?.microphone_status ?? "checking"}
          action={
            !permissions?.microphone ? (
              <button type="button" onClick={onRequestMicrophone} className={compactButtonClass}>
                {t(permissions?.microphone_status === "denied" || permissions?.microphone_status === "restricted" ? "打开设置" : "开启权限")}
              </button>
            ) : undefined
          }
        />
        <PermissionRow
          icon={<TextCursorInput {...iconProps} aria-hidden="true" />}
          kind="accessibility"
          title="自动粘贴（可选）"
          ok={permissions?.accessibility ?? false}
          status={permissions?.accessibility ? "authorized" : "pending"}
          action={
            !permissions?.accessibility ? (
              <button type="button" onClick={onEnableAccessibility} className={compactButtonClass}>
                {t("开启权限")}
              </button>
            ) : undefined
          }
        />
      </div>

      <div className="mt-4 flex items-center justify-between gap-3">
        <p className="text-xs leading-relaxed text-tertiary">
          {t("没有自动粘贴权限也没关系，结果会复制到剪贴板。")}
        </p>
        <button type="button" onClick={onRefresh} className={`${compactButtonClass} shrink-0 gap-1.5`}>
          <RefreshCw {...iconPropsSm} aria-hidden="true" />
          {t("重新检测")}
        </button>
      </div>

      {settingsError && (
        <p role="alert" className="mt-2 text-xs text-error">
          {settingsError}
        </p>
      )}
    </div>
  );
}
