import {
  HugeiconsIcon,
  type HugeiconsIconProps,
  type IconSvgElement,
} from "@hugeicons/react";
import {
  Add01Icon,
  AppleIcon as AppleMark,
  ArrowDown01Icon,
  ArrowDown02Icon,
  ArrowMoveDownRightIcon,
  ArrowRight02Icon,
  ArrowUp02Icon,
  ArrowUpRight01Icon,
  AudioWave01Icon,
  BookOpen02Icon,
  Cancel01Icon,
  GithubIcon as GithubMark,
  LockKeyholeIcon,
  Mail01Icon,
  Menu02Icon,
  Message01Icon,
  MoreHorizontalIcon,
  Notion01Icon,
  PauseIcon,
  PlayIcon,
  RefreshCcwIcon,
  SlackIcon,
  SourceCodeIcon,
  SourceCodeSquareIcon,
  Tick02Icon,
  VisualStudioCodeIcon,
  WechatIcon,
} from "@hugeicons/core-free-icons";

type IconProps = Omit<HugeiconsIconProps, "icon">;

function createIcon(icon: IconSvgElement) {
  return function SiteIcon({ size = 18, className = "", ...props }: IconProps) {
    return (
      <HugeiconsIcon
        icon={icon}
        size={size}
        strokeWidth={1.5}
        color="currentColor"
        className={`site-icon ${className}`.trim()}
        aria-hidden="true"
        focusable="false"
        {...props}
      />
    );
  };
}

export const Add = createIcon(Add01Icon);
export const AppleIcon = createIcon(AppleMark);
export const ArrowDown = createIcon(ArrowDown02Icon);
export const ArrowRight = createIcon(ArrowRight02Icon);
export const ArrowUp = createIcon(ArrowUp02Icon);
export const ArrowUpRight = createIcon(ArrowUpRight01Icon);
export const AudioWave = createIcon(AudioWave01Icon);
export const Check = createIcon(Tick02Icon);
export const ChevronDown = createIcon(ArrowDown01Icon);
export const Code2 = createIcon(SourceCodeIcon);
export const CursorEditor = createIcon(SourceCodeSquareIcon);
export const Dictionary = createIcon(BookOpen02Icon);
export const GitHubIcon = createIcon(GithubMark);
export const LockKeyhole = createIcon(LockKeyholeIcon);
export const Mail = createIcon(Mail01Icon);
export const Menu = createIcon(Menu02Icon);
export const MessageCircle = createIcon(Message01Icon);
export const MoreHorizontal = createIcon(MoreHorizontalIcon);
export const Notion = createIcon(Notion01Icon);
export const Pause = createIcon(PauseIcon);
export const Play = createIcon(PlayIcon);
export const RotateCcw = createIcon(RefreshCcwIcon);
export const Slack = createIcon(SlackIcon);
export const SnippetArrow = createIcon(ArrowMoveDownRightIcon);
export const VisualStudioCode = createIcon(VisualStudioCodeIcon);
export const Wechat = createIcon(WechatIcon);
export const X = createIcon(Cancel01Icon);

export function FlowPath({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 120 160"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M8 22C64 5 24 116 76 84c22-14 5-48-6-30-14 24 4 58 41 61m-17-13 17 13-20 7"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function VoiceMark({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 180 84"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M4 42h24c14 0 14-27 28-27s14 53 28 53S99 7 114 7s17 35 31 35h31M147 65h29"
        stroke="currentColor"
        strokeWidth="6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
