import type { ReactNode } from "react";
import { useI18n } from "../../lib/i18n";
import { isProviderId } from "../../lib/providers";
import { ghostButtonClass } from "../../lib/theme";
import { retentionLabel } from "../../lib/retention";

export type OnboardingSettingsPage = "engine" | "recording" | "permissions" | "dictionary" | "context" | "system" | "history" | "writing";

export function DataChoices({ settings, onSettings }: { settings: Record<string, unknown>; onSettings: (page: OnboardingSettingsPage) => void }) {
  const { t } = useI18n();
  const provider = typeof settings.asr_provider === "string" && isProviderId(settings.asr_provider) ? settings.asr_provider : null;
  const enabledState = (value: unknown) => typeof value === "boolean" ? t(value ? "已开启" : "已关闭") : t("无法确认");
  const dictionaryLearnState = typeof settings.dictionary_learn_enabled !== "boolean"
    ? t("无法确认")
    : !settings.dictionary_learn_enabled
      ? t("已关闭")
      : typeof settings.local_edit_learning_enabled !== "boolean"
        ? t("已开启（本机改字观察状态未知）")
        : settings.local_edit_learning_enabled
          ? t("已开启（含本机改字观察）")
          : t("已开启（未开启本机改字观察）");
  const items: { title: string; value: ReactNode; note: string; page: OnboardingSettingsPage }[] = [
    { title: "音频去向", value: provider === null ? t("无法确认") : t(provider === "on_device" ? "本机识别" : "发送到所选转写服务"), note: "本机识别与云端整理是独立选择；严格离线模式另行控制网络请求。", page: "engine" },
    { title: "AI 文字整理", value: enabledState(settings.cleanup_enabled), note: "开启时，准备稿会交给你配置的本机或云端整理服务。关闭后不请求整理模型，本地规则仍生效。", page: "engine" },
    { title: "历史与恢复音频", value: <>{t("保留历史文字")}：{retentionLabel(settings.keep_history_days, "history", t)} · {t("保留音频")}：{retentionLabel(settings.keep_audio_days, "audio", t)}</>, note: "失败录音按保留策略用于恢复。历史重转写由你主动触发，结果只复制。", page: "recording" },
    { title: "成功录音保留", value: enabledState(settings.keep_success_audio), note: "默认关闭。保留录音不等于批准训练；自检不保存音频，也不自动成为训练标签。", page: "recording" },
    { title: "上下文", value: enabledState(settings.context_enabled), note: "默认听写不截图。云端视觉与风格样例需要单独批准。常规词典候选观察需要开启全局词典学习与上下文，并由「本机观察插入后的改动」开关或该 App 规则单独授权的辅助功能文字读取允许。", page: "context" },
    { title: "词典学习", value: dictionaryLearnState, note: "词典学习是可审查的候选与替换规则，不是微调模型权重（含全局规则与本机私密规则选择）。默认仅从历史对比或手动添加生成规则。实时观察只在全局词典学习与上下文均开启、且目标 App 允许学习时发生：可由「本机观察插入后的改动」开关授权；也可由该 App 规则单独授权的辅助功能文字读取授权。前者无需专门创建映射；后者仅作用于对应 App。不会记录全局输入文字。", page: "dictionary" },
  ];
  return <details className="mt-6 border-t border-border pt-4">
    <summary className="cursor-pointer text-sm font-medium">{t("隐私与数据选择")}</summary>
    <p className="mt-3 text-xs leading-5 text-secondary">{t("以下是当前保存的设置。API Key 保存在这台 Mac 的钥匙串；你可以随时更改选择。")}</p>
    <dl className="mt-3 space-y-4">{items.map((item) => <div key={item.title}>
      <dt className="text-sm font-medium">{t(item.title)} · <span className="text-secondary">{item.value}</span></dt>
      <dd className="mt-1 text-xs leading-5 text-secondary">{t(item.note)} <button type="button" className={ghostButtonClass} onClick={() => onSettings(item.page)}>{t("前往设置")}</button></dd>
    </div>)}</dl>
  </details>;
}
