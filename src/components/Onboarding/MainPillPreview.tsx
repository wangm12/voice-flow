import { useI18n } from "../../lib/i18n";
import { VoicePill } from "../Island/VoicePill";
import { voicePillWindowHeight, voicePillWindowWidth } from "../Island/voicePillTokens";
import "../../island.css";

export function MainPillPreview({ live = false }: { live?: boolean }) {
  const { t } = useI18n();

  return (
    <div
      className="voice-pill-stage mx-auto"
      inert
      aria-hidden="true"
      style={{ width: voicePillWindowWidth, height: voicePillWindowHeight }}
    >
      <VoicePill
        state={live ? "recording" : "processing"}
        phase={live ? "idle" : "cleanup"}
        phaseConfirmed={!live}
        audioStatus={t("预设演示 · 未使用麦克风")}
        audioDeviceName={t("演示输入设备")}
        audioDeviceSourceLabel={t("预览")}
        audioDeviceTitle={t("演示输入设备；未使用麦克风")}
        contextLabel={null}
        selectedActionState={null}
        waveformLevels={[]}
        lastAudioLevelAtMs={null}
        clockNowMs={0}
        stageElapsedMs={0}
        reduced
      />
    </div>
  );
}
