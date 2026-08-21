import { ArrowUp, X } from "lucide-react";
import { VoiceWaveform } from "../Island/VoiceWaveform";
import {
  voicePillHeight,
  voicePillWidthForState,
  voicePillWindowHeight,
  voicePillWindowWidth,
} from "../Island/voicePillTokens";
import "../../island.css";

export function MainPillPreview({ live = false, compact = false }: { live?: boolean; compact?: boolean }) {
  const pillWidth = voicePillWidthForState("recording") - (compact ? 12 : 0);

  return (
    <div
      className="voice-pill-stage mx-auto"
      style={{ width: voicePillWindowWidth, height: voicePillWindowHeight }}
    >
      <div
        className={`voice-pill${compact ? " voice-pill--onboarding-compact" : ""}`}
        style={{ width: pillWidth, height: voicePillHeight }}
      >
        <span className="voice-pill__side voice-pill__side--cancel" aria-hidden="true">
          <X size={13} strokeWidth={2.25} absoluteStrokeWidth />
        </span>
        <div className="voice-pill__content">
          <VoiceWaveform dim={!live} active={live} preview={live} />
        </div>
        <span className="voice-pill__side voice-pill__side--action" aria-hidden="true">
          <ArrowUp size={14} strokeWidth={2.35} absoluteStrokeWidth />
        </span>
      </div>
    </div>
  );
}
