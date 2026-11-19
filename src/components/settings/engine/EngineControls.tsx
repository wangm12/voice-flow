import { Autocomplete } from "../../Autocomplete";
import { PasswordInput } from "../../PasswordInput";
import { Select } from "../../Select";
import { useI18n } from "../../../lib/i18n";
import { defaultModel, providerById, type ModelOption, type ProviderId } from "../../../lib/providers";
import type { OnDeviceModelStatus } from "../../../types/settings";
import { controlClass } from "./engineHelpers";

export function ModelControl({
  id,
  side,
  value,
  ariaLabel,
  onChange,
  deviceModels = [],
  optionsOverride,
}: {
  id: ProviderId;
  side: "asr" | "llm";
  value: string;
  ariaLabel: string;
  onChange: (value: string) => void;
  deviceModels?: OnDeviceModelStatus[];
  optionsOverride?: readonly ModelOption[];
}) {
  const { t } = useI18n();
  const definition = providerById(id);
  if (!definition) return null;
  const field = side === "asr" ? definition.asrModelField : definition.llmModelField;
  const options = optionsOverride ?? (side === "asr" ? definition.asrModels : definition.llmModels);
  if (id === "on_device_llama_cpp") {
    const selectedValue = options.some((option) => option.value === value) ? value : "";
    return (
      <Select
        aria-label={ariaLabel}
        value={selectedValue}
        onValueChange={(next) => onChange(next)}
        disabled={options.length === 0}
        className={`${controlClass} w-[220px] max-w-full`}
      >
        {selectedValue === "" && <option value="" disabled>{t("请选择已下载并验证的模型")}</option>}
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={Boolean(option.retiredForNewSelection && option.value !== value)}
          >{option.note ? `${option.label} · ${t(option.note)}` : option.label}</option>
        ))}
      </Select>
    );
  }
  if (field === "select" && options.length > 0) {
    return (
      <Select aria-label={ariaLabel} value={value} onValueChange={(value) => onChange(value)} className={`${controlClass} w-[220px] max-w-full`}>
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={Boolean(option.retiredForNewSelection && option.value !== value)
              || (id === "on_device"
                && option.value !== "sensevoice-small"
                && deviceModels.find((model) => model.id === option.value)?.platform_supported === false)}
          >
            {option.note ? `${option.label} · ${t(option.note)}` : option.label}
          </option>
        ))}
      </Select>
    );
  }
  return (
    <>
      <Autocomplete
        aria-label={ariaLabel}
        value={value}
        options={options}
        onValueChange={onChange}
        placeholder={defaultModel(id, side === "asr" ? "asr" : "llm") || "model"}
        autoComplete="off"
        spellCheck={false}
        className={`${controlClass} w-[220px] max-w-full font-mono text-sm`}
      />

    </>
  );
}

export function SecretField({
  id,
  ariaLabel,
  typed,
  hint,
  placeholder,
  onTypedChange,
}: {
  id: string;
  ariaLabel: string;
  typed: string;
  hint: string;
  placeholder: string;
  onTypedChange: (value: string) => void;
}) {
  const showingHint = !typed && Boolean(hint);
  return (
    <PasswordInput
      id={id}
      ariaLabel={ariaLabel}
      value={typed || hint}
      onChange={(value) => {
        if (showingHint && value === hint) return;
        onTypedChange(value === hint ? "" : value);
      }}
      placeholder={placeholder}
      monospace
      plain={showingHint}
      className="mt-2"
    />
  );
}
