import type { ButtonHTMLAttributes, ReactNode } from "react";
import { colors, controlSize, focusRingClass, radius } from "../lib/theme";

type IconButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  label: string;
  icon: ReactNode;
  size?: "sm" | "md";
  tone?: "default" | "warning" | "danger";
  tooltip?: boolean;
  unstyled?: boolean;
};

const toneClasses = {
  default: `${colors.text.tertiary} enabled:hover:bg-elevated enabled:hover:text-primary enabled:active:bg-border enabled:active:text-primary`,
  warning: "text-warning-ink enabled:hover:bg-warning/10 enabled:hover:text-warning-ink enabled:active:bg-warning/15",
  danger: "text-tertiary enabled:hover:bg-error/10 enabled:hover:text-error-ink enabled:active:bg-error/15 enabled:active:text-error-ink",
} as const;

/** A consistent icon-only action with an accessible label and visible hover name. */
export function IconButton({
  label,
  icon,
  size = "md",
  tone = "default",
  tooltip = true,
  unstyled = false,
  className = "",
  type = "button",
  "aria-label": ariaLabel,
  title,
  ...props
}: IconButtonProps) {
  const sizeClass = size === "sm" ? controlSize.iconSm : controlSize.icon;
  const classes = unstyled
    ? className
    : [
      "vf-icon-button",
      "inline-flex shrink-0 items-center justify-center",
      sizeClass,
      radius.control,
      toneClasses[tone],
      `transition-colors duration-150 motion-reduce:transition-none ${focusRingClass} disabled:cursor-not-allowed disabled:opacity-50`,
      className,
    ].filter(Boolean).join(" ");

  return (
    <button
      {...props}
      type={type}
      aria-label={ariaLabel ?? label}
      title={title ?? label}
      data-tooltip={tooltip ? label : undefined}
      className={classes}
    >
      {icon}
    </button>
  );
}
