import { MainPillPreview } from "./MainPillPreview";
import { ParticleText } from "../ReactBits/ParticleText";
import { secondaryButtonClass } from "../../lib/theme";
import { DataChoices, type OnboardingSettingsPage } from "./DataChoices";
import { useI18n } from "../../lib/i18n";
import { Check } from "lucide-react";

export function WelcomeStep({ settings, route, onRoute, onSettings }: { settings: Record<string, unknown>; route: "cloud" | "local"; onRoute: (route: "cloud" | "local") => void; onSettings: (page: OnboardingSettingsPage) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex min-h-[420px] flex-col justify-center">
      <div className="mb-14 flex h-[68px] w-full items-center justify-center">
        <MainPillPreview />
      </div>
      <div className="w-full max-w-[620px]">
        <p className="mb-3 text-xs text-tertiary">{t("状态条演示；尚未开始录音。")}</p>
        <div aria-hidden="true" className="space-y-1">
          <ParticleText text={t("不再打字。")} className="vf-particle-text--title" />
          <ParticleText text={t("让 VoiceFlow 处理剩下的。")} className="vf-particle-text--title" />
        </div>
        <h1 className="sr-only">{t("不再打字。")} {t("让 VoiceFlow 处理剩下的。")}</h1>
        <p className="mt-5 text-[13px] leading-6 text-secondary">{t("按快捷键说话，在应用内试用；自动粘贴和上下文由你选择。")}</p>
        <div role="group" aria-label={t("选择首次使用路线")} className="mt-6 flex flex-wrap gap-3">
          <button type="button" aria-pressed={route === "cloud"} className={`${secondaryButtonClass} ${route === "cloud" ? "bg-accent-soft text-primary" : ""}`} onClick={() => onRoute("cloud")}><Check size={14} aria-hidden="true" className={route === "cloud" ? "" : "invisible"} />{t("自选云端服务")}</button>
          <button type="button" aria-pressed={route === "local"} className={`${secondaryButtonClass} ${route === "local" ? "bg-accent-soft text-primary" : ""}`} onClick={() => onRoute("local")}><Check size={14} aria-hidden="true" className={route === "local" ? "" : "invisible"} />{t("本机识别")}</button>
        </div>
        <p className="mt-2 text-xs leading-5 text-secondary">{t("聊天、邮件和技术输入都可以使用；选择路线不会授权读取任何 App。")}</p>
        <DataChoices settings={settings} onSettings={onSettings} />
      </div>
    </div>
  );
}
