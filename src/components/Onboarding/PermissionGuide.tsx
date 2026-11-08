/* Adapted from Handy AccessibilityOnboarding.tsx (MIT), commit
 * 14f6f0d31cb22a4268acfbddba3052820fd0a06e, permission waiting/granted card.
 * Copyright (c) 2025 CJ Pais. Full notice: ./HANDY-LICENSE.txt. */
import { Check, Loader2, TextCursorInput, X } from "lucide-react";
import { useI18n } from "../../lib/i18n";
import { compactButtonClass, secondaryButtonClass } from "../../lib/theme";
import type { NativePermissionGuide } from "./usePermissionGuide";

export function PermissionGuide({ granted, nativeGuide, guideFailed, onRefresh, onOpenSettings, onRetryGuide, onClose }: { granted: boolean; nativeGuide?: NativePermissionGuide | null; guideFailed?: boolean; onRefresh: () => void; onOpenSettings: () => void; onRetryGuide?: () => void; onClose: () => void }) {
  const { t } = useI18n();
  return <aside aria-label={t("自动粘贴授权提示")} className="mt-4 rounded-lg border border-border bg-elevated p-4">
    <div className="flex items-center justify-between gap-3"><h2 className="text-sm font-medium">{t("在辅助功能中允许 VoiceFlow")}</h2><button type="button" aria-label={t("关闭提示")} className={compactButtonClass} onClick={onClose}><X size={16} aria-hidden="true" /></button></div>
    <div className="mt-3 flex items-start gap-3"><TextCursorInput size={24} aria-hidden="true" /><div>
      <p className="text-sm font-medium">VoiceFlow</p>
      <p className="mt-1 text-xs leading-5 text-secondary">{t("在系统设置 → 隐私与安全性 → 辅助功能中找到 VoiceFlow，开启右侧开关。")}</p>
      <p className="mt-2 text-xs leading-5 text-secondary">{t("找不到条目时，点击列表下方的添加按钮，在应用程序中选择 VoiceFlow。开发版本请选当前运行的 App。")}</p>
      {nativeGuide?.window_open && <p className="mt-2 text-xs leading-5 text-secondary">{t(nativeGuide.drag_available ? "可移动原生提示窗已打开；可把其中的 App 文件卡片拖入系统列表，或使用添加按钮。拖拽完成不代表已授权。" : "原生提示窗已打开；当前运行环境无法提供 App 文件拖拽，请使用系统添加按钮或 Finder 路径。")}</p>}
      {nativeGuide?.bundle_path && <p className="mt-2 break-all text-xs text-secondary">{t("当前运行的 App")}：{nativeGuide.bundle_path}</p>}
      {guideFailed && <p role="alert" className="mt-2 text-xs text-error-ink">{t("原生权限提示窗状态无法确认；可重新打开，或按上面的系统添加路径操作。")}</p>}
      <p className="mt-2 flex items-center gap-2 text-xs" role="status">{granted ? <Check size={16} aria-hidden="true" /> : <Loader2 size={16} className="text-secondary" aria-hidden="true" />}{t(granted ? "本机权限检查已通过" : "等待本机权限检查；打开设置不代表已授权。")}</p>
    </div></div>
    <div className="mt-3 flex flex-wrap gap-2"><button type="button" className={secondaryButtonClass} onClick={onOpenSettings}>{t("打开辅助功能设置")}</button>{onRetryGuide && !granted && <button type="button" className={compactButtonClass} onClick={onRetryGuide}>{t("重新打开权限提示窗")}</button>}<button type="button" className={compactButtonClass} onClick={onRefresh}>{t("重新检测")}</button><button type="button" className={compactButtonClass} onClick={onClose}>{t("稍后开启，先使用复制")}</button></div>
  </aside>;
}
