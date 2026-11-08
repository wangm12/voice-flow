import { useI18n } from "../../lib/i18n";
import { compactButtonClass } from "../../lib/theme";
import { ContextualGuide } from "./ContextualGuide";

export function DictionaryLesson({ onAddTerm, onOpenHistory, onOpenPreferences }: {
  onAddTerm?: () => void; onOpenHistory?: () => void; onOpenPreferences?: () => void;
}) {
  const { t } = useI18n();
  return <ContextualGuide guideId="dictionary_learning" title={t("词典与纠错：从添加到真正使用")}>
    <p>{t("手动术语示例：星澜协议。把你自己的词条填入“添加词条”并确认；添加只是配置，不代表已经识别正确。")}</p>
    <p>{t("错词示例：星蓝协议 → 星澜协议。可在历史中编辑结果、保存，再审查给出的词典建议；示例本身不会加入词典。")}</p>
    <ol className="list-decimal space-y-2 pl-5">
      <li>{t("日常学习：在 VoiceFlow 刚插入文字的同一输入框内纠正错词。继续打字不会被当作纠错。")}</li>
      <li>{t("系统先观察，再区分人名、术语或不适合学习的改动；有些改动不会成为候选。待确认列表为空不表示已经学会。")}</li>
      <li>{t("同一听写只计一次：人名需 2 次、普通词需 3 次不同听写中的纠正后自动生效；也可在待确认列表手动确认。候选、已生效替换和实际替换次数是不同事实。")}</li>
      <li>{t("学习规则按记录的 App 与场景范围筛选；当前 App、网站、映射或任务信息缺失或不匹配时不应用，目标与任务保护仍然生效。")}</li>
    </ol>
    <p>{t("“忽略”或“忘记”会阻止同一条学习规则再次自动积累；删除手动词条与撤销学习规则是不同操作。固定的词条仍可在列表中手动管理。")}</p>
    <p>{t("请用新句子检查是否真正替换，再看不该替换的句子、重启和其他 App。候选数量与替换次数都不是准确率。日常纠错不会自动把录音用于模型训练。")}</p>
    <div className="flex flex-wrap gap-2">
      {onAddTerm && <button type="button" className={compactButtonClass} onClick={onAddTerm}>{t("添加自己的术语")}</button>}
      {onOpenHistory && <button type="button" className={compactButtonClass} onClick={onOpenHistory}>{t("去历史纠正文字")}</button>}
      {onOpenPreferences && <button type="button" className={compactButtonClass} onClick={onOpenPreferences}>{t("查看 App 学习与风格规则")}</button>}
    </div>
  </ContextualGuide>;
}

export function ContextLesson({ onOpenPermissions, onOpenMappings, firstUse = false }: {
  onOpenPermissions?: () => void; onOpenMappings?: () => void; firstUse?: boolean;
}) {
  const { t } = useI18n();
  return <ContextualGuide guideId="context_screen" title={t("上下文与看屏幕：分别选择来源和用途")} firstUse={firstUse}>
    <p>{t("默认听写不截图。普通 App 适配、自动上下文和手动看屏幕是不同选择；阅读本说明不会采集屏幕或开启权限。")}</p>
    <ul className="list-disc space-y-2 pl-5">
      <li>{t("辅助功能文字：允许读取这条规则匹配的 App 文字；系统辅助功能权限只是前提，不等于允许所有 App 内容。")}</li>
      <li>{t("本机 OCR：从获准窗口提取文字，需要相应系统权限与 OCR 开关；图片不因此获得云端使用许可。")}</li>
      <li>{t("上下文文字发给服务商：另行允许把获准提取的文字用于整理请求；读取来源和发送用途要分别选择。")}</li>
      <li>{t("云端视觉：另行允许窗口图片交给视觉模型，必须绑定具体 App 或可执行文件；网站条件本身不足以授权截图。")}</li>
      <li>{t("风格样例：只有单独批准并保存的样例才可用于整理服务；待确认草稿不是批准。")}</li>
    </ul>
    <p>{t("在语气页的 App 规则中，修改开关后需保存才生效。系统权限已允许、规则已配置和本次确实捕获成功，需要分别确认。撤销某项来源不会自动撤销其他独立授权。")}</p>
    <p>{t("手动看屏幕：设置独立快捷键和视觉模型后，在目标窗口主动触发。结果先预览；目标变化时只复制，取消丢弃本次内存图片，不保存截图到历史。")}</p>
    <div className="flex flex-wrap gap-2">
      {onOpenMappings && <button type="button" className={compactButtonClass} onClick={onOpenMappings}>{t("查看 App 来源规则")}</button>}
      {onOpenPermissions && <button type="button" className={compactButtonClass} onClick={onOpenPermissions}>{t("查看系统权限（不自动申请）")}</button>}
    </div>
  </ContextualGuide>;
}

export function SelectedLesson({ firstUse = false }: { firstUse?: boolean }) {
  const { t } = useI18n();
  return <ContextualGuide guideId="selected_translation" title={t("选中文本与翻译：先选来源，再确认结果")} firstUse={firstUse}>
    <p>{t("固定练习原稿（仅为示例）：周五下午三点确认 API 方案。可选任务：“翻译成英文”、“缩短”或“改写得更专业”。示例不是模型结果，也不会自动执行。")}</p>
    <p>{t("整理示例（仅为说明）：“整理成开发任务”/“整理成工作进展”/“整理成待办”。示例不是模型结果；只加固定标题并按段落列出，保留原文用词、标点和顺序，不补写未提供的信息。打开说明不会读取选区或请求模型。")}</p>
    <ol className="list-decimal space-y-2 pl-5">
      <li>{t("在首次设置的可选试用框练习，或回到原 App 重新选择文字，再按独立文字操作快捷键说出任务。打开说明不会沿用旧选区执行。")}</li>
      <li>{t("核对预览中的操作、来源和结果，可编辑、只复制或取消。应用内试用只更新试用框；外部操作需在预览中主动确认。")}</li>
      <li>{t("没有选区时回原输入框重新选择；权限不足时先检查辅助功能。目标或来源变化时不覆盖新内容，可用已保留的结果手动复制。")}</li>
    </ol>
    <p>{t("取消或失败后不要把迟到结果当作新任务成功。重新选择来源再触发；若识别失败，可在确有恢复录音的历史记录中重试，重试只复制，不恢复旧目标。")}</p>
    <p>{t("翻译听写使用另一独立快捷键与目标语言；它翻译本次录音，不改日常输出模式。文字操作与回复草稿都不会替你发送消息或按 Enter。")}</p>
  </ContextualGuide>;
}

export function SceneLesson({ onOpenDictionary }: { onOpenDictionary?: () => void }) {
  const { t } = useI18n();
  return <ContextualGuide guideId="scene_preferences" title={t("语气与个人偏好：草稿、保存和批准")}>
    <p>{t("示例偏好（仅为说明）：工作回复简洁礼貌，保留 API、数字与日期。请把自己的要求写入现有语气 Prompt；草稿不会切换当前听写语气，保存后才成为配置。")}</p>
    <p>{t("用下方固定原稿主动点击试跑。有未保存修改时可比较已保存语气与当前草稿；试跑可能请求已配置的整理服务，不读取私人历史或当前窗口。示例结果不是当前授权或外部写入的证明。")}</p>
    <p>{t("App、可执行文件、网站、路径和输入框条件需要同时匹配；把偏好保存到对应规则。终端、代码和安全输入继续遵循保守保护，风格要求不能覆盖事实保护。")}</p>
    <p>{t("纠错形成的风格草稿在个人词典中待确认；确认时会明确批准这组样例用于对应 App 的整理请求。手动编辑的样例需单独打开批准开关并保存。取消“允许风格示例发送给整理服务商”并保存，可停止后续使用；也可删除整条 App 规则。旧预览不表示当前仍获准。")}</p>
    <p>{t("不需要连接或导入其他 AI 账户。词条、Prompt、App 规则和批准样例分别由现有设置管理。")}</p>
    {onOpenDictionary && <button type="button" className={compactButtonClass} onClick={onOpenDictionary}>{t("审查待确认风格草稿")}</button>}
  </ContextualGuide>;
}
