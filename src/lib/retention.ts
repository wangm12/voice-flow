export const audioRetentionOptions = [
  { value: 0, label: "立即清理" }, { value: 1, label: "1 天" },
  { value: 7, label: "1 周" }, { value: 30, label: "1 月" }, { value: 365, label: "1 年" },
] as const;

export const historyRetentionOptions = [
  { value: 7, label: "1 周" }, { value: 30, label: "1 月" }, { value: 90, label: "3 月" },
  { value: 365, label: "1 年" }, { value: 3650, label: "10 年" }, { value: 0, label: "永久保留，直到手动删除" },
] as const;

export function retentionLabel(value: unknown, kind: "history" | "audio", t: (source: string) => string): string {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) return t("无法确认");
  const options = kind === "history" ? historyRetentionOptions : audioRetentionOptions;
  const option = options.find((item) => item.value === value);
  return option ? t(option.label) : t("{days} 天").replace("{days}", String(value));
}
