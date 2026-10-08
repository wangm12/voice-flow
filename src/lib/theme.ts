export const colors = {
  bg: {
    base: "bg-base",
    card: "bg-card",
    elevated: "bg-elevated",
  },
  text: {
    primary: "text-primary",
    secondary: "text-secondary",
    tertiary: "text-tertiary",
  },
  border: "border-border",
  accent: {
    text: "text-accent",
    background: "bg-action",
    foreground: "text-action-foreground",
  },
  semantic: {
    success: "text-success-ink",
    warning: "text-warning-ink",
    error: "text-error-ink",
    successBackground: "bg-success",
    warningBackground: "bg-warning",
    errorBackground: "bg-error",
  },
} as const;

export const radius = {
  card: "rounded-2xl",
  control: "rounded-xl",
  pill: "rounded-full",
} as const;

export const controlSize = {
  button: "h-10",
  tag: "h-7",
  input: "h-10",
  icon: "h-10 w-10",
  iconSm: "h-8 w-8",
} as const;

export const focusRingClass =
  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-0";

const buttonBase =
  `vf-button inline-flex shrink-0 items-center justify-center gap-2 font-medium transition-colors duration-150 motion-reduce:transition-none ${focusRingClass} disabled:cursor-not-allowed`;

export const buttonClass = `${radius.control} ${controlSize.button} ${buttonBase} text-sm border border-action-border ${colors.accent.background} ${colors.accent.foreground} px-3 enabled:hover:bg-action-hover enabled:active:bg-action-pressed`;

export const secondaryButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} text-sm border border-transparent ${colors.bg.elevated} px-3 ${colors.text.secondary} enabled:hover:bg-accent-soft enabled:hover:text-primary enabled:active:bg-border enabled:active:text-primary`;

export const ghostButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} text-sm px-3 ${colors.text.secondary} enabled:hover:text-primary enabled:hover:bg-elevated enabled:active:bg-border enabled:active:text-primary`;

export const compactButtonClass = `${radius.control} h-8 ${buttonBase} px-3 text-xs border border-transparent ${colors.bg.elevated} ${colors.text.secondary} enabled:hover:bg-accent-soft enabled:hover:text-primary enabled:active:bg-border enabled:active:text-primary`;

export const dangerButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} text-sm border border-error-ink bg-error/5 px-3 text-error-ink enabled:hover:bg-error/10 enabled:active:bg-error/15`;

export const dangerActionButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} text-sm border border-error-ink bg-danger-action px-3 text-danger-action-foreground enabled:hover:bg-danger-action-hover enabled:active:bg-danger-action-pressed`;

export const compactDangerButtonClass = `${radius.control} h-8 ${buttonBase} text-xs border border-error-ink bg-error/5 px-3 text-error-ink enabled:hover:bg-error/10 enabled:active:bg-error/15`;

export const inputClass = `vf-input ${radius.control} ${controlSize.input} min-w-0 border border-transparent bg-elevated px-3 text-sm text-primary placeholder:text-tertiary outline-none transition-colors duration-150 ${focusRingClass} disabled:cursor-not-allowed`;

export const tagClass = `${radius.pill} inline-flex h-7 items-center gap-1.5 px-2.5 text-xs font-medium`;

export const linkButtonClass = `${colors.accent.text} transition-opacity duration-150 hover:underline underline-offset-4 ${focusRingClass}`;

export function validationMessage(status: string, translate: (source: string) => string = (source) => source): string {
  switch (status) {
    case "valid":
      return translate("访问密钥有效");
    case "invalid":
      return translate("访问密钥无效");
    case "rate_limited":
      return translate("请求过频，请稍后重试");
    case "server_error":
      return translate("语音服务暂时不可用，请稍后重试");
    case "network_error":
      return translate("网络错误，无法验证；请检查网络或代理");
    case "ipc_error":
      return translate("验证失败，请重启应用后重试");
    default:
      return translate("验证失败，请重试");
  }
}

export function validationTone(status: string): string {
  if (status === "valid") return colors.semantic.success;
  if (status === "invalid") return colors.semantic.error;
  return colors.semantic.warning;
}
