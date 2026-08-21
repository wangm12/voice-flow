export function friendlySettingsError(
  reason: unknown,
  translate: (source: string) => string,
): string {
  const message = reason instanceof Error ? reason.message : String(reason);
  const normalized = message.toLowerCase();
  if (
    normalized.includes("credential_storage")
    || normalized.includes("failed to store api key securely")
    || normalized.includes("credential write")
    || normalized.includes("keychain")
  ) {
    return translate("无法保存到这台 Mac 的钥匙串。请重启 VoiceFlow 后再试。");
  }
  if (normalized.includes("api key validation failed") || normalized.includes("invalid")) {
    return translate("这个 Groq API Key 无效，请检查后重试。");
  }
  if (normalized.includes("rate_limited") || normalized.includes("请求过频")) {
    return translate("验证请求过频，请稍后再试。");
  }
  if (normalized.includes("network") || normalized.includes("timeout")) {
    return translate("暂时无法连接 Groq，请检查网络后重试。");
  }
  return translate(message);
}
