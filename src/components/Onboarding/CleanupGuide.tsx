import { useState } from "react";
import { useI18n } from "../../lib/i18n";
import { Select } from "../Select";
import { WritingPreview } from "../WritingPreview";
import type { WritingMode } from "../ContextSettings";
import { cleanupIntentView, isCleanupIntensity, type CleanupIntensity, type OnboardingCleanupIntent } from "../../lib/onboardingCleanup";

export function CleanupGuide({ settings, intent, onSave }: { settings: Record<string, unknown>; intent: OnboardingCleanupIntent; onSave: (patch: { cleanup_enabled: boolean; cleanup_intensity: CleanupIntensity }) => Promise<void> }) {
  const { t } = useI18n();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { intensity, pendingLocalDefault } = cleanupIntentView(settings, intent);
  const modes = Array.isArray(settings.writing_modes) ? settings.writing_modes as WritingMode[] : [];
  const previewMode = modes.find((mode) => mode && typeof mode.id === "string" && typeof mode.label === "string" && typeof mode.prompt === "string" && mode.prompt.trim());
  const change = async (value: string) => {
    if (!isCleanupIntensity(value)) return;
    setSaving(true); setError(null);
    try { await onSave({ cleanup_enabled: value !== "off", cleanup_intensity: value }); }
    catch { setError(t("整理设置保存失败，原选择已保留。")); }
    finally { setSaving(false); }
  };
  return <details className="mt-5 border-t border-border pt-4">
    <summary className="cursor-pointer text-sm font-medium">{t("整理强度与保真")}</summary>
    <p className="mt-3 text-xs leading-5 text-secondary">{t("Auto 按场景选择；Off 不请求整理模型；Light 窄幅清理；Standard 常规整理；Heavy 才附加润色。App 规则的显式选择优先。")}</p>
    {pendingLocalDefault && <p className="mt-2 text-xs text-secondary">{t("新选择本机路线默认关闭 AI 整理，继续时应用。你也可以主动选择整理强度；重新检查已有路线不会重置已保存选择。")}</p>}
    <p className="mt-2 text-xs text-secondary">{t("已配置整理服务")}：{String(settings.cleanup_provider ?? t("无法确认"))} / {String(settings.cleanup_model ?? t("无法确认"))}</p>
    <label className="mt-3 block text-xs font-medium">{t("AI 整理强度")}<Select aria-label={t("AI 整理强度")} value={intensity} disabled={saving} onValueChange={(value) => void change(value)} className="mt-1 h-9 w-full rounded-lg border border-border bg-elevated px-3 text-sm"><option value="auto">Auto</option><option value="off">Off</option><option value="light">Light</option><option value="standard">Standard</option><option value="heavy">Heavy</option></Select></label>
    <p className="mt-2 text-xs leading-5 text-secondary">{t("数字、否定、路径和有意强调需要保留。保护检查拒绝或服务失败时保留准备稿，预览会显示实际回退。")}</p>
    {pendingLocalDefault && <p className="mt-2 text-xs text-secondary">{t("默认选择尚未保存；继续应用或明确保存整理选择后，再使用真实配置试跑。")}</p>}
    {!pendingLocalDefault && previewMode && <WritingPreview mode={previewMode} compareSaved={false} isDraft={false} />}
    {error && <p role="alert" className="mt-2 text-xs text-error-ink">{error}</p>}
  </details>;
}
