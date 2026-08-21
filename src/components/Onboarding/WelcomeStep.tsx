import { MainPillPreview } from "./MainPillPreview";
import { ParticleText } from "../ReactBits/ParticleText";
import { useI18n } from "../../lib/i18n";

export function WelcomeStep() {
  const { t } = useI18n();
  return (
    <div className="flex min-h-[420px] flex-col justify-center">
      <div className="mb-14 flex h-[60px] w-full items-center justify-center">
        <MainPillPreview live compact />
      </div>
      <div className="w-full max-w-[620px]">
        <div className="space-y-1">
          <ParticleText text={t("不再打字。")} className="vf-particle-text--title" />
          <ParticleText text={t("让 VoiceFlow 处理剩下的。")} className="vf-particle-text--title" />
        </div>
        <ParticleText
          text={t("VoiceFlow 会根据你正在使用的 App，整理你说的话。")}
          className="vf-particle-text--subtitle mt-5"
          delay={500}
          particleScale={0.75}
        />
      </div>
    </div>
  );
}
