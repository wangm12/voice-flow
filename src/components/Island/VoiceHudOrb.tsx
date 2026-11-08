import { memo } from "react";
import { ThinkingOrb, type OrbState } from "thinking-orbs";
import { HUD_ORB_SIZE } from "./hudOrb";

export const VoiceHudOrb = memo(function VoiceHudOrb({
  state,
  paused,
  speed,
  level = 0,
  dim = false,
  caution = false,
}: {
  state: OrbState;
  paused: boolean;
  speed: number;
  level?: number;
  dim?: boolean;
  caution?: boolean;
}) {
  return (
    <span
      className={[
        "voice-pill__orb",
        dim ? "voice-pill__orb--dim" : "",
        caution ? "voice-pill__orb--caution" : "",
      ].filter(Boolean).join(" ")}
      data-orb-state={state}
      data-orb-paused={paused ? "true" : "false"}
      data-orb-speed={speed.toFixed(2)}
      data-audio-level={Math.max(0, Math.min(1, level)).toString()}
      aria-hidden="true"
    >
      <span
        className="voice-pill__level-ring"
        style={{
          transform: `scale(${0.84 + Math.max(0, Math.min(1, level)) * 0.3})`,
          opacity: level > 0 ? 0.18 + Math.max(0, Math.min(1, level)) * 0.66 : 0,
        }}
      />
      <ThinkingOrb
        state={state}
        size={HUD_ORB_SIZE}
        theme="dark"
        paused={paused}
        speed={speed}
        aria-hidden="true"
      />
    </span>
  );
});
