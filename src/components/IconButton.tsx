import type { ButtonHTMLAttributes, ReactNode } from "react";
import { colors, controlSize, radius } from "../lib/theme";

type IconButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  label: string;
  icon: ReactNode;
  size?: "sm" | "md";
  tone?: "default" | "warning" | "danger";
  tooltip?: boolean;
  unstyled?: boolean;
};

const toneClasses = {
  default: `${colors.text.tertiary} hover:bg-elevated hover:text-primary`,
  warning: "text-warning hover:bg-warning/10 hover:text-warning",
  danger: "text-tertiary hover:bg-error/10 hover:text-error",
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
      "transition-colors duration-150 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:opacity-50",
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
