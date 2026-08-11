export const voicePillHeight = 34;
// Keep the transparent native surface close to the visible HUD. A large
// transparent WebView still participates in WindowServer composition.
export const voicePillWindowWidth = 172;
export const voicePillWindowHeight = 60;

/** Compact fixed width; state labels ellipsize inside the center slot. */
export function voicePillWidthForState(state: string): number {
  void state;
  return 132;
}

export function pillCaption(state: string, translate: (source: string) => string = (source) => source): string | null {
  switch (state) {
    case "recording":
      return null;
    case "processing":
      return null;
    case "recording_limited":
      return translate("已达到 15 分钟上限，请按热键结束");
    case "rate_limited":
      return translate("处理时间比平时长…");
    case "degraded":
      return translate("部分结果已保存，请检查后再使用");
    case "error":
      return translate("语音输入失败，请重试");
    default:
      return null;
  }
}
