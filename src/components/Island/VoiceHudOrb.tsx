import { memo } from "react";
import { ThinkingOrb, type OrbState } from "thinking-orbs";
import { HUD_ORB_SIZE } from "./hudOrb";

export const VoiceHudOrb = memo(function VoiceHudOrb({
  state,
  paused,
  speed,
  dim = false,
  caution = false,
}: {
  state: OrbState;
  paused: boolean;
  speed: number;
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
      aria-hidden="true"
    >
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
