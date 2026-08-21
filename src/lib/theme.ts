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
    background: "bg-accent",
    foreground: "text-accent-foreground",
  },
  semantic: {
    success: "text-success",
    warning: "text-warning",
    error: "text-error",
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

export const shadow = {
  card: "shadow-card",
  elevated: "shadow-elevated",
} as const;

export const easing = {
  emphasized: "cubic-bezier(0.2, 0, 0, 1)",
} as const;

export const controlSize = {
  button: "h-9",
  tag: "h-7",
  input: "h-9",
  icon: "h-9 w-9",
  iconSm: "h-7 w-7",
} as const;

export const cssVariables = {
  colors: {
    base: "var(--color-base)",
    card: "var(--color-card)",
    elevated: "var(--color-elevated)",
    primary: "var(--color-primary)",
    secondary: "var(--color-secondary)",
    tertiary: "var(--color-tertiary)",
    border: "var(--color-border)",
    accent: "var(--color-accent)",
    success: "var(--color-success)",
    warning: "var(--color-warning)",
    error: "var(--color-error)",
  },
  shadow: {
    card: "var(--shadow-card)",
    elevated: "var(--shadow-elevated)",
  },
  easing: {
    emphasized: "var(--ease-emphasized)",
  },
} as const;

export const textClass = {
  title: "text-2xl font-semibold tracking-tight text-primary",
  subtitle: "text-sm text-secondary",
  body: "text-sm font-normal text-primary",
  caption: "text-xs font-normal text-tertiary",
} as const;

export const focusRingClass =
  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";

const buttonBase =
  `inline-flex items-center justify-center gap-1.5 text-sm font-medium transition-colors duration-150 active:scale-[0.98] motion-reduce:active:scale-100 ${focusRingClass} disabled:cursor-not-allowed`;

export const buttonClass = `${radius.control} ${controlSize.button} ${buttonBase} ${colors.accent.background} ${colors.accent.foreground} px-4 hover:opacity-90 disabled:opacity-40`;

export const secondaryButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} border ${colors.border} ${colors.bg.card} px-4 ${colors.text.secondary} hover:bg-elevated hover:text-primary disabled:opacity-30`;

export const ghostButtonClass = `${radius.control} ${controlSize.button} ${buttonBase} px-3 ${colors.text.tertiary} hover:text-primary hover:bg-elevated/60`;

export const compactButtonClass = `${radius.control} ${controlSize.button} inline-flex items-center justify-center gap-1 px-3 text-xs font-medium border ${colors.border} ${colors.text.secondary} transition-colors hover:bg-elevated hover:text-primary ${focusRingClass}`;

export const tagClass = `${radius.pill} inline-flex h-7 items-center gap-1.5 px-2.5 text-xs font-medium`;

export const iconBoxClass = `flex ${controlSize.icon} shrink-0 items-center justify-center rounded-xl border ${colors.border} ${colors.bg.elevated} ${colors.text.primary}`;

export const linkButtonClass = `${colors.accent.text} transition-opacity duration-150 hover:opacity-80`;

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
