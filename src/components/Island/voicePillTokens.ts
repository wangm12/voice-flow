export const voicePillWidth = 220;
export const voicePillHeight = 48;
/** Native host stays centered on the fixed primary capsule. */
export const voicePillWindowWidth = 244;
export const voicePillWindowHeight = 64;
/** Details/recovery sits above the fixed primary capsule in the native host. */
export const voicePillRecoveryWidth = 320;
export const voicePillRecoveryHeight = 116;
export const voicePillLearnToastHeight = 48;

export const selectedActionCaptionLabels: Record<string, string> = {
  waiting_for_selection: "等待选中文本",
  listening: "正在听取操作",
  preparing_rewrite: "正在准备改写",
  accessibility_required: "需要辅助功能权限",
  replaced: "已替换选中文本",
  copied_instead: "目标变化，结果已复制",
};

export function selectedActionCaption(
  state: string | null | undefined,
  translate: (source: string) => string = (source) => source,
): string | null {
  if (!state) return null;
  const label = selectedActionCaptionLabels[state];
  return label ? translate(label) : null;
}
