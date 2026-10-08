import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import { inputClass } from "../lib/theme";
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
  plain?: boolean;
};

export function PasswordInput({ value, onChange, id, ariaLabel, placeholder, className = "", valid = false, monospace = false, plain = false }: PasswordInputProps) {
  const { t } = useI18n();
  const [visible, setVisible] = useState(false);
  const accessibleName = ariaLabel ?? t("API Key");

  return (
    <div className={`relative ${className}`}>
      <input
        id={id}
        aria-label={accessibleName}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onFocus={(event) => {
          if (plain) event.currentTarget.select();
        }}
        type={plain || visible ? "text" : "password"}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        data-valid={valid || undefined}
        className={`${inputClass} w-full pr-10 ${monospace ? "font-mono tracking-tight" : ""}` }
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
