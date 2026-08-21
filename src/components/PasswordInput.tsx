import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import { colors, controlSize, radius, focusRingClass } from "../lib/theme";
import { iconPropsSm } from "../lib/icons";
import { IconButton } from "./IconButton";
import { useI18n } from "../lib/i18n";

type PasswordInputProps = {
  value: string;
  onChange: (value: string) => void;
  id?: string;
  ariaLabel?: string;
  placeholder?: string;
  className?: string;
  valid?: boolean;
  monospace?: boolean;
};

export function PasswordInput({ value, onChange, id, ariaLabel, placeholder, className = "", valid = false, monospace = false }: PasswordInputProps) {
  const { t } = useI18n();
  const [visible, setVisible] = useState(false);
  const [focused, setFocused] = useState(false);
  const accessibleName = ariaLabel ?? t("API Key");

  return (
    <div className={`relative ${className}`}>
      <input
        id={id}
        aria-label={accessibleName}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        type={visible ? "text" : "password"}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        className={`w-full ${controlSize.input} ${radius.control} border py-0 pl-3.5 pr-10 text-sm ${colors.text.primary} outline-none transition-[border-color,background-color,box-shadow] duration-200 ${focusRingClass} ${
          valid
            ? "border-success/60 bg-success/5 shadow-[0_0_0_3px_rgb(16_185_129_/_0.12)]"
            : focused
              ? `border-accent ${colors.bg.elevated} shadow-[0_0_0_3px_rgb(24_24_27_/_0.08)]`
              : `${colors.border} ${colors.bg.elevated}`
        } ${monospace ? "font-mono tracking-tight" : ""}`}
      />
      <IconButton
        size="sm"
        label={visible ? t("隐藏 API Key") : t("显示 API Key")}
        icon={visible ? <EyeOff {...iconPropsSm} /> : <Eye {...iconPropsSm} />}
        onClick={() => setVisible((current) => !current)}
        className="absolute right-1 top-1/2 z-10 -translate-y-1/2"
      />
    </div>
  );
}
