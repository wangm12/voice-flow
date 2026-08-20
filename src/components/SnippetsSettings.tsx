import { useState } from "react";
import { Trash2 } from "lucide-react";
import { colors, radius, buttonClass } from "../lib/theme";
import { useI18n } from "../lib/i18n";
import { SettingsGroup, SettingsPageHeader, SettingsShell } from "./SettingsLayout";
import { ConfirmDialog } from "./ConfirmDialog";
import { Toggle } from "./Toggle";

export type Snippet = {
  id: string;
  trigger: string;
  expansion: string;
  enabled: boolean;
};

function newId() {
  return `snippet.${Date.now()}.${Math.random().toString(36).slice(2, 8)}`;
}

export function SnippetsSettings({ snippets, onChange }: { snippets: Snippet[]; onChange: (snippets: Snippet[]) => void }) {
  const { t } = useI18n();
  const [trigger, setTrigger] = useState("");
  const [expansion, setExpansion] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);

  const add = () => {
    const nextTrigger = trigger.trim().replace(/\s+/g, " ");
    const nextExpansion = expansion.trim();
    if (!nextTrigger || !nextExpansion) {
      setError(t("请填写触发短语和展开内容"));
      return;
    }
    if (snippets.some((item) => item.trigger.trim().replace(/\s+/g, " ").toLowerCase() === nextTrigger.toLowerCase())) {
      setError(t("这个触发短语已经存在"));
      return;
    }
    onChange([...snippets, { id: newId(), trigger: nextTrigger, expansion: nextExpansion, enabled: true }]);
    setTrigger("");
    setExpansion("");
    setError(null);
  };

  return (
    <SettingsShell>
      <SettingsPageHeader title={t("语音片段")} description={t("说出完整触发短语，就能插入一段本地保存的文字。不会上传到 Groq。")} />
      <SettingsGroup title={t("添加片段")} description={t("只匹配整段短录音，普通句子不会误触发。")}>
        <form
          onSubmit={(event) => { event.preventDefault(); add(); }}
          className="px-4 py-4 sm:px-5"
        >
          <div className="grid gap-4 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]">
            <label className="min-w-0">
              <span className="text-sm font-medium text-primary">{t("触发短语")}</span>
              <span className="mt-1 block text-xs leading-5 text-tertiary">{t("例如：插入我的邮箱")}</span>
              <input
                aria-label={t("触发短语")}
                value={trigger}
                onChange={(event) => setTrigger(event.target.value)}
                placeholder={t("说出的完整短语…")}
                className={`mt-3 w-full ${radius.control} h-9 border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 text-sm outline-none transition-colors focus:border-accent`}
              />
            </label>
            <label className="min-w-0">
              <span className="text-sm font-medium text-primary">{t("展开内容")}</span>
              <span className="mt-1 block text-xs leading-5 text-tertiary">{t("支持纯文本或本地模板")}</span>
              <textarea
                aria-label={t("展开内容")}
                value={expansion}
                onChange={(event) => setExpansion(event.target.value)}
                rows={3}
                placeholder={t("要插入的文字…")}
                className={`mt-3 min-h-20 w-full resize-y ${radius.control} border ${colors.border} ${colors.bg.elevated} ${colors.text.primary} px-3 py-2 text-sm outline-none transition-colors focus:border-accent`}
              />
            </label>
          </div>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
            {error ? <p role="alert" className="text-xs text-error">{error}</p> : <p className="text-xs text-tertiary">{snippets.length >= 128 ? t("已达到片段上限") : t("添加后，说出完整短语即可展开")}</p>}
            <button type="submit" disabled={snippets.length >= 128} className={buttonClass}>{t("添加片段")}</button>
          </div>
        </form>
      </SettingsGroup>
      <SettingsGroup title={t("已保存片段")} description={`${snippets.length} ${t("条")}`}>
        {snippets.length === 0 ? <p className="px-4 py-5 text-sm text-tertiary sm:px-5">{t("还没有语音片段。")}</p> : snippets.map((snippet) => (
          <div key={snippet.id} className="flex flex-wrap items-start gap-4 px-4 py-3.5 sm:px-5">
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium text-primary">{snippet.trigger}</p>
              <p className="mt-1 whitespace-pre-wrap break-words text-xs text-tertiary">{snippet.expansion}</p>
            </div>
            <div className="flex shrink-0 items-center gap-3">
              <Toggle
                checked={snippet.enabled}
                onChange={(enabled) => onChange(snippets.map((item) => item.id === snippet.id ? { ...item, enabled } : item))}
                label={`${t("启用")} ${snippet.trigger}`}
              />
              <button type="button" aria-label={`${t("删除片段")} ${snippet.trigger}`} onClick={() => setPendingDeleteId(snippet.id)} className="rounded-lg p-2 text-tertiary transition-colors hover:bg-error/10 hover:text-error"><Trash2 size={15} aria-hidden="true" /></button>
            </div>
          </div>
        ))}
      </SettingsGroup>
      <ConfirmDialog
        open={pendingDeleteId != null}
        title={t("删除片段")}
        description={t("确定删除“{name}”吗？").replace("{name}", snippets.find((item) => item.id === pendingDeleteId)?.trigger ?? "")}
        confirmLabel={t("删除")}
        cancelLabel={t("取消")}
        onCancel={() => setPendingDeleteId(null)}
        onConfirm={() => {
          onChange(snippets.filter((item) => item.id !== pendingDeleteId));
          setPendingDeleteId(null);
        }}
      />
    </SettingsShell>
  );
}
