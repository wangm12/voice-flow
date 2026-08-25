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
});
