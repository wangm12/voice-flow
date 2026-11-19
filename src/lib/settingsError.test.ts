import { describe, expect, it } from "vitest";
import { friendlySettingsError, isDictionaryPartialSyncError, settingsErrorCode } from "./settingsError";

const identity = (source: string) => source;

describe("friendlySettingsError", () => {
  it("maps keychain failures", () => {
    expect(
      friendlySettingsError(
        new Error("credential_storage: failed to store API key securely: keychain write timed out"),
        identity,
      ),
    ).toBe("无法保存到这台 Mac 的钥匙串。请重启 VoiceFlow 后再试。");
  });

  it("keeps engine validation messages from objects or strings", () => {
    expect(friendlySettingsError("自定义 ASR 地址需要填写 ASR 密钥。", identity))
      .toBe("自定义 ASR 地址需要填写 ASR 密钥。");
    expect(friendlySettingsError({ message: "自定义整理地址需要填写整理密钥。" }, identity))
      .toBe("自定义整理地址需要填写整理密钥。");
  });

  it("preserves the actionable partial dictionary-sync message", () => {
    expect(friendlySettingsError("learning_dictionary_partial:settings_save_failed", identity))
      .toBe("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
  });

  it("maps dictionary capacity errors before the generic invalid-key fallback", () => {
    expect(friendlySettingsError("dictionary_capacity_or_invalid_word", identity))
      .toBe("词条无效或已达到上限。");
  });

  it("maps hotkey, url, and general validation errors separately from api key errors", () => {
    expect(friendlySettingsError("invalid hotkey combination", identity))
      .toBe("快捷键组合无效，请重新设置。");
    expect(friendlySettingsError("translation hotkey conflicts with dictation", identity))
      .toBe("翻译快捷键与其他快捷键冲突，请选择不同的组合键。");
    expect(friendlySettingsError("invalid custom_base_url: missing host", identity))
      .toBe("服务地址格式无效，请检查后重试。");
    expect(friendlySettingsError("error sending request for url (https://api.groq.com/openai/v1/models): operation timeout", identity))
      .toBe("暂时无法连接服务，请检查网络后重试。");
    expect(friendlySettingsError("recognition language 'ar' is not supported by the selected ASR model", identity))
      .toBe("当前所选识别语言不受当前转写模型支持，请改选支持列表中的语言。");
    expect(friendlySettingsError("preferred_languages contains an invalid language code", identity))
      .toBe("常用口述语言设置无效（最多选 4 项，不能包含自动识别或重复语言）。");
    expect(friendlySettingsError("unsupported preferred language: xx", identity))
      .toBe("常用口述语言设置无效（最多选 4 项，不能包含自动识别或重复语言）。");
    expect(friendlySettingsError("duplicate preferred language: zh", identity))
      .toBe("常用口述语言设置无效（最多选 4 项，不能包含自动识别或重复语言）。");
    expect(friendlySettingsError("API key validation failed (401)", identity))
      .toBe("这个 API Key 无效，请检查后重试。");
    expect(friendlySettingsError("input_monitoring_required", identity))
      .toBe("Fn 快捷键需要输入监控权限。请在系统设置的隐私与安全性中为 VoiceFlow 开启输入监控。");
    expect(friendlySettingsError("modifier_event_tap_unavailable", identity))
      .toBe("Fn 按键监听暂不可用；请检查输入监控权限后重试。");
  });

  it("prefers typed SettingsCommandError codes over message substring matching", () => {
    expect(settingsErrorCode({ code: "hotkey_conflict", message: "raw error" })).toBe("hotkey_conflict");
    expect(settingsErrorCode({ code: "unknown_code", message: "raw error" })).toBeNull();
    expect(isDictionaryPartialSyncError({ code: "dictionary_partial_sync", message: "opaque" })).toBe(true);
    expect(friendlySettingsError({ code: "dictionary_partial_sync", message: "opaque" }, identity))
      .toBe("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
    expect(friendlySettingsError({ code: "dictionary_capacity_or_invalid_word", message: "opaque" }, identity))
      .toBe("词条无效或已达到上限。");
    expect(friendlySettingsError({ code: "hotkey_conflict", message: "opaque" }, identity))
      .toBe("快捷键与其他快捷键冲突，请选择不同的组合键。");
    expect(friendlySettingsError({ code: "translation_hotkey_invalid", message: "opaque" }, identity))
      .toBe("翻译快捷键需要有效的组合键，请重新设置。");
    expect(friendlySettingsError({ code: "invalid_endpoint_url", message: "opaque" }, identity))
      .toBe("服务地址格式无效，请检查后重试。");
    expect(friendlySettingsError({ code: "retired_on_device_asr_model", message: "opaque" }, identity))
      .toBe("Cohere Transcribe 2B 已停止提供新选择；已有保存配置可以继续保留。");
  });
});
