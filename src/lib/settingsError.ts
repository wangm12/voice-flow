function settingsErrorMessage(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (reason instanceof Error) return reason.message;
  if (reason && typeof reason === "object" && "message" in reason) {
    const message = (reason as { message: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return String(reason);
}

export function friendlyDictionaryLearningError(
  reason: unknown,
  translate: (source: string) => string,
): string | null {
  const message = settingsErrorMessage(reason);
  if (message.toLowerCase().startsWith("learning_dictionary_partial:")) {
    return translate("学习规则或词条来源已记录，但个人词典同步未完成。请重试。");
  }
  if (message === "dictionary_capacity_or_invalid_word") {
    return translate("词条无效或已达到上限。");
  }
  return null;
}

export function friendlySettingsError(
  reason: unknown,
  translate: (source: string) => string,
): string {
  const message = settingsErrorMessage(reason);
  const normalized = message.toLowerCase();
  const dictionaryMessage = friendlyDictionaryLearningError(reason, translate);
  if (dictionaryMessage) return dictionaryMessage;
  if (normalized === "retired_on_device_asr_model") {
    return translate("Cohere Transcribe 2B 已停止提供新选择；已有保存配置可以继续保留。");
  }
  if (normalized.includes("translation hotkey")) {
    return translate(normalized.includes("conflicts") ? "翻译快捷键与其他快捷键冲突，请选择不同的组合键。" : "翻译快捷键需要有效的组合键，请重新设置。");
  }
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
