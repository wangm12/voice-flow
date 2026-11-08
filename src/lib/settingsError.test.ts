import { describe, expect, it } from "vitest";
import { friendlySettingsError } from "./settingsError";

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
});
