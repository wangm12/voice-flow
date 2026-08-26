import { VoiceHudOrb } from "../Island/VoiceHudOrb";
import { BorderBeam } from "border-beam";
import {
  HUD_LISTENING_BEAM,
  HUD_LISTENING_ORB_SPEED,
  HUD_PILL_RADIUS_PX,
  HUD_THINKING_BEAM,
  HUD_THINKING_ORB_SPEED,
} from "../Island/hudOrb";
import {
  voicePillHeight,
  voicePillWidthForState,
  voicePillWindowHeight,
  voicePillWindowWidth,
} from "../Island/voicePillTokens";
import "../../island.css";

export function MainPillPreview({ live = false, compact = false }: { live?: boolean; compact?: boolean }) {
  const pillWidth = voicePillWidthForState("recording") - (compact ? 8 : 0);
  const beam = live ? HUD_LISTENING_BEAM : HUD_THINKING_BEAM;

  return (
    <div
      className="voice-pill-stage mx-auto"
      style={{ width: voicePillWindowWidth, height: voicePillWindowHeight }}
    >
      <BorderBeam
        className="voice-pill__beam"
        size={beam.size}
        colorVariant={beam.colorVariant}
        strength={beam.strength}
        duration={beam.duration}
        theme="dark"
        borderRadius={HUD_PILL_RADIUS_PX}
        active
        style={{ width: pillWidth, height: voicePillHeight }}
      >
        <div
          className={`voice-pill voice-pill--labeled voice-pill--orb${compact ? " voice-pill--onboarding-compact" : ""}`}
          style={{ width: pillWidth, height: voicePillHeight }}
        >
          <div className="voice-pill__content">
            <span className="voice-pill__indicator">
              <span className="voice-pill__center-state voice-pill__center-state--active" aria-hidden="true">
                <VoiceHudOrb
                  state={live ? "breathing" : "shaping"}
                  paused={!live}
                  speed={live ? HUD_LISTENING_ORB_SPEED : HUD_THINKING_ORB_SPEED}
                  dim={!live}
                />
              </span>
            </span>
            <span className="voice-pill__label" aria-hidden="true">
              {live ? "Listening…" : "Thinking…"}
            </span>
          </div>
        </div>
      </BorderBeam>
    </div>
  );
}
