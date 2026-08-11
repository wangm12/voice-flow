import type { ActivationMode } from "../lib/activationCopy";
import { formatHotkeyDisplay, isModifierOnlyHotkey, sanitizeTauriHotkey } from "../lib/hotkeyFormat";
import { radius } from "../lib/theme";
import { useI18n } from "../lib/i18n";

type UsageRow = {
  gesture: string;
  trigger: string;
  stop: string;
  preferred?: boolean;
};

function modifierRows(t: (source: string) => string): UsageRow[] {
  return [{ gesture: t("双击功能键"), trigger: t("开始录音"), stop: t("再双击 → 结束并转换成文字"), preferred: true }];
}

function comboRows(hotkeyDisplay: string, t: (source: string) => string): UsageRow[] {
  return [{ gesture: `${t("按")} ${hotkeyDisplay}`, trigger: t("开始录音"), stop: `${t("再按一次")} ${hotkeyDisplay} → ${t("结束并转换成文字")}`, preferred: true }];
}

export function hotkeyUsageRows(hotkey: string, _activationMode?: ActivationMode | string, translate: (source: string) => string = (source) => source): UsageRow[] {
  const normalized = sanitizeTauriHotkey(hotkey);
  const display = formatHotkeyDisplay(normalized);
  if (isModifierOnlyHotkey(normalized)) {
    return modifierRows(translate);
  }
  return comboRows(display, translate);
}

export function HotkeyUsageGuide({
  hotkey,
  activationMode,
}: {
  hotkey: string;
  activationMode?: ActivationMode | string;
}) {
  const { t } = useI18n();
  if (!hotkey.trim()) return null;

  const rows = hotkeyUsageRows(hotkey, activationMode, t);
  const isModifier = isModifierOnlyHotkey(hotkey);
  const row = rows[0];
  if (!row) return null;

  return (
    <div className={`mt-4 ${radius.control} border border-border bg-card/60 px-3.5 py-3`}>
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <span className="text-[11px] font-semibold uppercase tracking-[0.14em] text-tertiary">{t("使用方式")}</span>
        {row.preferred && <span className="rounded-full bg-accent/10 px-2 py-1 text-[10px] font-medium text-secondary">{t("当前快捷键")}</span>}
      </div>
      <div className="mt-3 grid gap-2.5 text-sm sm:grid-cols-3 sm:gap-3">
        <div className="flex min-w-0 items-start gap-2">
          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-elevated text-[11px] font-semibold text-secondary">1</span>
          <span className="min-w-0 leading-5 text-secondary">{row.gesture} · {row.trigger}</span>
        </div>
        <div className="flex min-w-0 items-start gap-2">
          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-elevated text-[11px] font-semibold text-secondary">2</span>
          <span className="min-w-0 leading-5 text-secondary">{row.stop}</span>
        </div>
        <div className="flex min-w-0 items-start gap-2">
          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-elevated text-[11px] font-semibold text-secondary">3</span>
          <span className="min-w-0 leading-5 text-secondary">{t("按 Esc 取消")}</span>
        </div>
      </div>
      {isModifier && <p className="mt-2.5 text-xs leading-5 text-tertiary">{t("功能键需要双击；组合快捷键则按一下切换。")}</p>}
    </div>
  );
}
