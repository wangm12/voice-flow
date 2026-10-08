import { Select } from "../Select";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SettingsGroup, SettingsPageHeader, SettingsRow, SettingsShell, SettingsStatus } from "../SettingsLayout";
import { Toggle } from "../Toggle";
import { MicrophoneCheck } from "../MicrophoneCheck";
import { type UiLanguagePreference, useI18n } from "../../lib/i18n";
import { inputClass } from "../../lib/theme";
import type { AudioInputDevice, SaveSettings, Settings } from "../../types/settings";

const controlClass = inputClass;

export function SystemSettings({
  settings,
  audioInputDevice,
  audioInputDevices,
  save,
  onUiLanguageChange,
}: {
  settings: Settings;
  audioInputDevice: string | null;
  audioInputDevices: AudioInputDevice[];
  save: SaveSettings;
  onUiLanguageChange: (uiLanguage: UiLanguagePreference) => void;
}) {
  const { t } = useI18n();
  const [autostartStatus, setAutostartStatus] = useState<{ enabled: boolean; error: string | null } | null>(null);
  useEffect(() => {
    let active = true;
    const refresh = () => {
      if (document.visibilityState !== "visible") return;
      void invoke<{ enabled: boolean; error: string | null }>("get_autostart_status")
        .then((status) => { if (active) setAutostartStatus(status); })
        .catch((error: unknown) => { if (active) setAutostartStatus({ enabled: false, error: String(error) }); });
    };
    refresh();
    const timer = window.setInterval(refresh, 1000);
    window.addEventListener("focus", refresh);
    return () => { active = false; window.clearInterval(timer); window.removeEventListener("focus", refresh); };
  }, [settings.autostart_enabled]);
  const selectedInputDevice = settings.input_device?.trim() ?? "";
  const selectedInputDeviceAvailable = !selectedInputDevice
    || audioInputDevices.some((device) => device.name === selectedInputDevice);
  const inputDeviceLabel = selectedInputDevice || audioInputDevice;
  const selectedClamshellDevice = settings.clamshell_microphone?.trim() ?? "";
  const selectedClamshellDeviceAvailable = !selectedClamshellDevice
    || audioInputDevices.some((device) => device.name === selectedClamshellDevice);

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("系统设置")} description={t("主题、语言、菜单栏图标和音频输入。")} />
      <SettingsGroup title={t("外观")}>
        <SettingsRow title={t("主题")} description={t("设置窗口使用浅色、深色，或跟随 macOS。")}>
          <Select aria-label={t("主题")} value={settings.theme} onValueChange={(value) => save({ theme: value as Settings["theme"] })} className={`${controlClass} w-40 max-w-full`}>
            <option value="system">{t("跟随系统")}</option>
            <option value="light">{t("浅色")}</option>
            <option value="dark">{t("深色")}</option>
          </Select>
        </SettingsRow>
        <SettingsRow title={t("语言")} description={t("设置窗口和界面文案的语言。")}>
          <Select aria-label={t("语言")} value={settings.ui_language} onValueChange={(value) => onUiLanguageChange(value as UiLanguagePreference)} className={`${controlClass} w-40 max-w-full`}>
            <option value="system">{t("跟随系统")}</option>
            <option value="zh">{t("中文")}</option>
            <option value="en">{t("English")}</option>
          </Select>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title={t("音频输入")} description={t("选择录音使用的麦克风；默认跟随 macOS 系统设置。")}>
        <SettingsRow title={t("输入设备")} description={<>
          <span>{t("默认会跟随 macOS 当前输入设备；选择具体设备后，录音会固定使用它。")}</span>
          {inputDeviceLabel && <span className="mt-2 block break-words text-xs leading-[18px] text-secondary">{t(selectedInputDevice ? "选定麦克风" : "当前麦克风")} · {inputDeviceLabel}</span>}
        </>}>
          <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
            <Select data-size="wide" aria-label={t("输入设备")} value={selectedInputDevice} onValueChange={(value) => save({ input_device: value })} className={`${controlClass} w-[220px] max-w-full`}>
              <option value="">{t("默认（跟随系统）")}</option>
              {selectedInputDevice && !selectedInputDeviceAvailable && <option value={selectedInputDevice}>{`${selectedInputDevice} · ${t("设备不可用")}`}</option>}
              {audioInputDevices.map((device) => <option key={device.name} value={device.name}>{device.name}</option>)}
            </Select>
            {selectedInputDevice && !selectedInputDeviceAvailable && <SettingsStatus label={t("设备不可用")} tone="warning" />}
          </div>
        </SettingsRow>
        <SettingsRow title={t("合盖麦克风")} description={t("笔记本合上盖时改用这只麦克风。")}>
          <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
            <Select data-size="wide" aria-label={t("合盖麦克风")} value={selectedClamshellDevice} onValueChange={(value) => save({ clamshell_microphone: value })} className={`${controlClass} w-[220px] max-w-full`}>
              <option value="">{t("关闭")}</option>
              {selectedClamshellDevice && !selectedClamshellDeviceAvailable && <option value={selectedClamshellDevice}>{`${selectedClamshellDevice} · ${t("设备不可用")}`}</option>}
              {audioInputDevices.map((device) => <option key={device.name} value={device.name}>{device.name}</option>)}
            </Select>
            {selectedClamshellDevice && !selectedClamshellDeviceAvailable && <SettingsStatus label={t("设备不可用")} tone="warning" />}
          </div>
        </SettingsRow>
        <div className="px-5"><MicrophoneCheck configurationKey={`${selectedInputDevice || audioInputDevice || ""}|${selectedClamshellDevice}|${settings.input_gain ?? 1}`} /></div>
      </SettingsGroup>
      <SettingsGroup title={t("应用行为")}>
        <SettingsRow title={t("菜单栏图标")} description={t("关闭后隐藏 VoiceFlow 的菜单栏图标；你仍可以从应用窗口重新打开设置。")}>
          <Toggle checked={settings.show_tray_icon} onChange={(checked) => save({ show_tray_icon: checked })} label={t("显示菜单栏图标")} />
        </SettingsRow>
        <SettingsRow title={t("开机启动")} description={t("登录 macOS 后在后台启动，不打开设置窗口。")}>
          <Toggle checked={autostartStatus?.enabled ?? false} onChange={(checked) => save({ autostart_enabled: checked })} label={t("开机启动")} />
        </SettingsRow>
        {autostartStatus?.error && <p role="alert" className="px-5 py-3 text-sm text-error-ink">{autostartStatus.error}</p>}
        {autostartStatus && !autostartStatus.error && autostartStatus.enabled !== (settings.autostart_enabled ?? false) && <p role="status" className="px-5 py-3 text-sm text-tertiary">{t("系统登录启动状态与设置不同，请重新切换开关。")}</p>}
      </SettingsGroup>
    </SettingsShell>
  );
}
