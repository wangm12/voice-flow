import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { useI18n } from "../../lib/i18n";
import { colors, focusRingClass, radius } from "../../lib/theme";
import type { AudioInputDevice, SaveSettings, Settings } from "../../types/settings";

const controlClass = `${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-0 text-sm outline-none transition-colors duration-150 focus:border-accent ${focusRingClass}`;

export function SystemSettings({
  settings,
  audioInputDevice,
  audioInputDevices,
  save,
}: {
  settings: Settings;
  audioInputDevice: string | null;
  audioInputDevices: AudioInputDevice[];
  save: SaveSettings;
}) {
  const { t } = useI18n();
  const selectedInputDevice = settings.input_device?.trim() ?? "";
  const selectedInputDeviceAvailable = !selectedInputDevice
    || audioInputDevices.some((device) => device.name === selectedInputDevice);

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("系统设置")} description={t("菜单栏图标、音频输入和系统级应用行为。")} />
      <SettingsGroup title={t("音频输入")} description={t("选择录音使用的麦克风；默认跟随 macOS 系统设置。")}>
        <SettingsRow title={t("输入设备")} description={t("默认会跟随 macOS 当前输入设备；选择具体设备后，录音会固定使用它。")}>
          <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
            <select aria-label={t("输入设备")} value={selectedInputDevice} onChange={(event) => save({ input_device: event.target.value })} className={`${controlClass} w-56 max-w-full`}>
              <option value="">{t("默认（跟随系统）")}{audioInputDevice ? ` · ${audioInputDevice}` : ""}</option>
              {selectedInputDevice && !selectedInputDeviceAvailable && <option value={selectedInputDevice}>{`${selectedInputDevice} · ${t("设备不可用")}`}</option>}
              {audioInputDevices.map((device) => <option key={device.name} value={device.name}>{device.name}</option>)}
            </select>
            {selectedInputDevice && !selectedInputDeviceAvailable && <SettingsStatus label={t("设备不可用")} tone="warning" />}
          </div>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("应用行为")}>
        <SettingsRow title={t("菜单栏图标")} description={t("关闭后隐藏 VoiceFlow 的菜单栏图标；你仍可以从应用窗口重新打开设置。")}>
          <Toggle checked={settings.show_tray_icon} onChange={(checked) => save({ show_tray_icon: checked })} label={t("显示菜单栏图标")} />
        </SettingsRow>
      </SettingsGroup>
    </SettingsShell>
  );
}
