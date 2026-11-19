import type { Dispatch, SetStateAction } from "react";
import { ChevronDown } from "lucide-react";
import { Select } from "../../Select";
import { SettingsDisclosure, SettingsGroup } from "../../SettingsLayout";
import { Toggle } from "../../Toggle";
import { useI18n } from "../../../lib/i18n";
import { parseCustomCleanupParameters } from "../../../lib/customCleanupParameters";
import { customHostChanged, customOriginChanged, type EngineDraft } from "../../../lib/engineWizard";
import { isLoopbackUrl, providerById, type DashscopeRegion, type ProviderId } from "../../../lib/providers";
import { compactButtonClass } from "../../../lib/theme";
import type { LocalCleanupStatus, Settings } from "../../../types/settings";
import { LatencyMetricsViewer } from "../LatencyMetricsViewer";
import { controlClass, type EnginePage } from "./engineHelpers";

export type EngineAdvancedTabProps = {
  page: EnginePage;
  pendingChanges: boolean;
  saveBlock: { page: EnginePage; target: string; message: string; action: string } | null;
  onNavigateToPage: (nextPage: EnginePage, targetId?: string) => void;
  settings: Settings;
  draft: EngineDraft;
  setDraft: Dispatch<SetStateAction<EngineDraft>>;
  draftCleanupEnabled: boolean;
  probing: boolean;
  rowProbing: ProviderId | null;
  removing: boolean;
  strictOfflineSaving: boolean;
  strictOfflineSaveError: string | null;
  onUpdateStrictOfflineMode: (enabled: boolean) => void;
  advancedConnectionsOpen: boolean;
  onAdvancedConnectionsOpenChange: (open: boolean) => void;
  onClearRowProbeFeedback: () => void;
  onClearProbeSuccess: () => void;
  onResetTestedAsrRoute: () => void;
  onResetTestedCleanupRoute: () => void;
  onChangeDashscopeRegion: (region: DashscopeRegion) => void;
  localCleanupChecking: boolean;
  localCleanupRouteSaved: boolean;
  localCleanupStatus: LocalCleanupStatus | null;
  onResetLocalCleanupStatus: () => void;
  localCleanupError: string | null;
  onCheckLocalCleanupStatus: () => void;
  onChooseLocalCleanup: () => void;
  metricsError: string | null;
  onMetricsErrorChange: (error: string | null) => void;
};

export function EngineAdvancedTab({
  page,
  pendingChanges,
  saveBlock,
  onNavigateToPage,
  settings,
  draft,
  setDraft,
  draftCleanupEnabled,
  probing,
  rowProbing,
  removing,
  strictOfflineSaving,
  strictOfflineSaveError,
  onUpdateStrictOfflineMode,
  advancedConnectionsOpen,
  onAdvancedConnectionsOpenChange,
  onClearRowProbeFeedback,
  onClearProbeSuccess,
  onResetTestedAsrRoute,
  onResetTestedCleanupRoute,
  onChangeDashscopeRegion,
  localCleanupChecking,
  localCleanupRouteSaved,
  localCleanupStatus,
  onResetLocalCleanupStatus,
  localCleanupError,
  onCheckLocalCleanupStatus,
  onChooseLocalCleanup,
  metricsError,
  onMetricsErrorChange,
}: EngineAdvancedTabProps) {
  const { t } = useI18n();
  const customParametersParse = parseCustomCleanupParameters(draft.customCleanupParametersText);

  return (
    <div
      id="engine-panel-advanced"
      role="region"
      aria-label={t("高级设置")}
      hidden={page !== "advanced"}
      inert={page !== "advanced"}
      className="vf-engine-page-content"
    >
      {!pendingChanges && saveBlock?.page === "advanced" && <p role="status" className="mb-4 flex flex-wrap items-center gap-2 text-xs leading-5 text-warning-ink"><span>{saveBlock.message}</span><button type="button" className={compactButtonClass} onClick={() => onNavigateToPage(saveBlock.page, saveBlock.target)}>{saveBlock.action}</button></p>}
      <SettingsGroup title={t("严格离线策略")} description={t("本机性能诊断只在本机显示。")}>
        <div id="advanced-strict-offline" className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-primary">{t("严格离线模式")} <span className="ml-1 text-xs font-normal text-tertiary">{t("立即生效")}</span></p>
            <p id="strict-offline-description" className="mt-1 text-xs leading-5 text-tertiary">{t("开启后会阻止云端回退和视觉请求；可继续使用经检查的本机 Ollama、VoiceFlow llama.cpp 或 VoiceFlow MLX 整理。本机模型下载仍是单独的联网操作。整理不可用时会回退到本地规则或关闭整理。")}</p>
            {strictOfflineSaveError && <p role="alert" className="mt-1 text-xs leading-5 text-error-ink">{strictOfflineSaveError}</p>}
          </div>
          <Toggle
            id="strict-offline-toggle"
            checked={settings.strict_offline_enabled === true}
            disabled={probing || rowProbing !== null || removing || strictOfflineSaving}
            onChange={onUpdateStrictOfflineMode}
            label={t("严格离线模式")}
            aria-describedby="strict-offline-description"
          />
        </div>
      </SettingsGroup>
      <section className="vf-settings-disclosure vf-settings-disclosure--surface vf-advanced-connections mt-6 rounded-xl border border-border bg-card">
        <fieldset disabled={probing || rowProbing !== null || removing || strictOfflineSaving} className="min-w-0">
          <details open={advancedConnectionsOpen} onToggle={(event) => onAdvancedConnectionsOpenChange(event.currentTarget.open)}>
            <summary id="advanced-connections-toggle" tabIndex={0} className="flex w-full cursor-pointer list-none items-center gap-3 rounded-xl px-5 py-4 text-left transition-colors enabled:hover:bg-elevated/40">
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium text-primary">{t("连接地址与高级请求参数")}</span>
                <span className="mt-1 block text-[13px] leading-5 text-secondary">{t("编辑自定义、本机服务地址、区域和整理请求参数；连接检查由你手动启动。")}</span>
              </span>
              <ChevronDown size={16} aria-hidden="true" className={`shrink-0 text-tertiary transition-transform duration-150 ${advancedConnectionsOpen ? "rotate-180" : ""}`} />
            </summary>
            <div className="vf-settings-disclosure-content space-y-4 border-t border-border px-5 pb-5 pt-4">
              <section id="advanced-custom-endpoint" className="space-y-3 rounded-lg bg-elevated p-4">
                <div>
                  <h3 className="text-sm font-medium text-primary">{t("自定义服务地址")}</h3>
                  <p className="mt-1 text-xs leading-5 text-tertiary">{t("转写和 AI 整理共用此地址与 API Key；两个阶段分别使用各自的模型。")}</p>
                </div>
                <label className="block text-xs text-secondary">
                  {t("自定义服务地址")}
                  <input
                    id="advanced-custom-base-url"
                    aria-label={t("自定义服务地址")}
                    value={draft.customBaseUrl}
                    onChange={(event) => {
                      const nextUrl = event.target.value;
                      const originChanged = customOriginChanged(draft.customBaseUrl, nextUrl);
                      setDraft((current) => ({
                        ...current,
                        customBaseUrl: nextUrl,
                        ...(originChanged ? { providerKeys: { ...current.providerKeys, custom: "" } } : {}),
                      }));
                      onClearRowProbeFeedback();
                      onClearProbeSuccess();
                      onResetTestedAsrRoute();
                      onResetTestedCleanupRoute();
                    }}
                    placeholder="https://api.example.com/v1"
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <div className="flex flex-wrap gap-2">
                  <button type="button" className={compactButtonClass} onClick={() => onNavigateToPage("api_key", "provider-key-custom")}>{t("管理凭据")}</button>
                </div>
                {customHostChanged(draft, settings) && !draft.providerKeys.custom?.trim() && !isLoopbackUrl(draft.customBaseUrl) && (
                  <p role="status" className="text-xs leading-5 text-warning-ink">{t("服务地址目标已更改；请填写新地址对应的 API Key。")}</p>
                )}
                <details>
                  <summary className="cursor-pointer text-xs font-medium text-secondary">{t("服务预设")}</summary>
                  <p className="mt-2 text-xs leading-5 text-tertiary">{t("预设会选择对应的转写服务、模型和地址；同一 origin 下保留候选密钥，跨 origin 时清除候选密钥。")}</p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {[
                      ["阿里云百炼 Qwen3-ASR", "https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen3-asr-flash"],
                      ["国际站 Qwen3-ASR", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", "qwen3-asr-flash"],
                      ["本机 FunASR", "http://127.0.0.1:10095/v1", "paraformer-zh"],
                      ["本机 Qwen3-ASR (MLX)", "http://127.0.0.1:8765/v1", "Qwen/Qwen3-ASR-0.6B"],
                    ].map(([label, url, model]) => (
                      <button key={label} type="button" className={compactButtonClass} onClick={() => {
                        setDraft((current) => ({
                          ...current,
                          asrProvider: "custom",
                          customAsr: true,
                          customBaseUrl: url,
                          asrModel: model,
                          ...(customOriginChanged(current.customBaseUrl, url) ? { providerKeys: { ...current.providerKeys, custom: "" } } : {}),
                        }));
                        onClearRowProbeFeedback();
                        onClearProbeSuccess();
                        onResetTestedAsrRoute();
                        onResetTestedCleanupRoute();
                      }}>{t(label)}</button>
                    ))}
                    <button type="button" className={compactButtonClass} onClick={() => {
                      setDraft((current) => ({ ...current, asrProvider: "local_whisper", localWhisperBaseUrl: "http://127.0.0.1:9000/v1", asrModel: "ggml-large-v3-turbo.bin" }));
                      onClearRowProbeFeedback();
                      onClearProbeSuccess();
                      onResetTestedAsrRoute();
                    }}>{t("本机 Whisper.cpp")}</button>
                  </div>
                </details>
              </section>

              {draft.asrProvider === "dashscope" && (
                <section id="advanced-dashscope-region" tabIndex={-1} className="rounded-lg bg-elevated p-4">
                  <label className="block text-xs font-medium text-secondary">
                    {t("DashScope 服务区域")}
                    <Select aria-label={t("服务区域")} value={draft.dashscopeRegion} onValueChange={(value) => onChangeDashscopeRegion(value as DashscopeRegion)} className={`${controlClass} mt-2 w-full text-xs`}>
                      <option value="beijing">{t("北京")} · dashscope.aliyuncs.com</option>
                      <option value="singapore">{t("新加坡")} · dashscope-intl.aliyuncs.com</option>
                    </Select>
                  </label>
                  <p className="mt-1 text-xs leading-5 text-tertiary">{t("地区会与转写服务设置一起保存；Message WebSocket 使用该地区的固定官方地址。")}</p>
                </section>
              )}

              <section id="advanced-local-whisper-connection" className="rounded-lg bg-elevated p-4">
                <label className="block text-xs text-secondary">
                  {t("本机 Whisper 地址")}
                  <input
                    id="advanced-local-whisper-url"
                    aria-label={t("本机 Whisper 地址")}
                    value={draft.localWhisperBaseUrl}
                    onChange={(event) => {
                      setDraft((current) => ({ ...current, localWhisperBaseUrl: event.target.value }));
                      onClearRowProbeFeedback();
                      onClearProbeSuccess();
                      onResetTestedAsrRoute();
                    }}
                    placeholder={providerById("local_whisper")?.defaultBaseUrl}
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <p className="mt-1 text-xs leading-5 text-tertiary">{t("先自行运行 whisper.cpp 或 speaches，并填写兼容的本机 HTTP 地址。")}</p>
              </section>

              <section id="advanced-ollama-connection" className="space-y-3 rounded-lg bg-elevated p-4">
                <label className="block text-xs text-secondary">
                  {t("Ollama 本机地址")}
                  <input
                    id="advanced-ollama-url"
                    aria-label={`Ollama ${t("本机地址")}`}
                    value={draft.ollamaBaseUrl}
                    onChange={(event) => {
                      setDraft((current) => ({ ...current, ollamaBaseUrl: event.target.value }));
                      onClearRowProbeFeedback();
                      onClearProbeSuccess();
                      onResetTestedCleanupRoute();
                      onResetLocalCleanupStatus();
                    }}
                    placeholder={providerById("ollama")?.defaultBaseUrl}
                    autoComplete="off"
                    spellCheck={false}
                    className={`${controlClass} mt-2 w-full font-mono text-xs`}
                  />
                </label>
                <p className="text-xs leading-5 text-tertiary">{t("VoiceFlow 不会安装 Ollama 或下载模型。检查状态只读取本机服务。")}</p>
                {localCleanupStatus && <p role="status" className={`text-xs leading-5 ${localCleanupStatus.available ? "text-success-ink" : "text-warning-ink"}`}>{localCleanupStatus.available ? t("本机整理服务可用") : t("本机整理服务不可用")} · {localCleanupStatus.status}{localCleanupStatus.message ? ` · ${localCleanupStatus.message}` : ""}</p>}
                {localCleanupError && <p role="alert" className="text-xs leading-5 text-error-ink">{localCleanupError}</p>}
                {!localCleanupRouteSaved && <p className="text-xs leading-5 text-warning-ink">{t("先保存本机 Ollama 地址，再检查状态。")}</p>}
                <div className="flex flex-wrap gap-2">
                  <button type="button" disabled={localCleanupChecking || !localCleanupRouteSaved} onClick={onCheckLocalCleanupStatus} className={compactButtonClass}>{localCleanupChecking ? t("检查中…") : t("检查本机 Ollama")}</button>
                  <button type="button" onClick={onChooseLocalCleanup} className={compactButtonClass}>{t("使用本机 Qwen3.5:4b 整理")}</button>
                </div>
              </section>

              {draftCleanupEnabled && draft.cleanupProvider === "custom" && (
                <section id="advanced-custom-cleanup-parameters-section" className="rounded-lg bg-elevated p-4">
                  <label className="block text-xs font-medium text-secondary">
                    {t("高级请求参数 JSON")}
                    <p className="mt-1 font-normal leading-5 text-tertiary">{t("填写 JSON 对象，作为自定义 OpenAI-compatible 整理请求的顶层字段。留空表示使用当前请求默认值；不要填写 API Key、密钥、headers、messages、model 或 stream。")}</p>
                    <textarea
                      id="advanced-custom-cleanup-parameters"
                      aria-label={t("高级请求参数 JSON")}
                      value={draft.customCleanupParametersText}
                      onChange={(event) => {
                        const source = event.target.value;
                        const parsed = parseCustomCleanupParameters(source);
                        setDraft((current) => ({
                          ...current,
                          customCleanupParametersText: source,
                          ...(parsed.ok ? { customCleanupParameters: parsed.value } : {}),
                        }));
                        onClearProbeSuccess();
                        onResetTestedCleanupRoute();
                      }}
                      rows={6}
                      spellCheck={false}
                      autoComplete="off"
                      className={`${controlClass} mt-2 min-h-28 w-full resize-y font-mono text-xs leading-5`}
                      placeholder={'{\n  "temperature": 0.2,\n  "max_completion_tokens": 2048\n}'}
                    />
                  </label>
                  {!customParametersParse.ok && <p role="alert" className="mt-2 text-xs leading-5 text-error-ink">{customParametersParse.error}</p>}
                </section>
              )}
            </div>
          </details>
        </fieldset>
      </section>
      <SettingsDisclosure title={t("本机性能诊断")} description={t("查看阶段延迟和交付计数，仅保存在本机。")} error={metricsError}>
        <LatencyMetricsViewer onErrorChange={onMetricsErrorChange} />
      </SettingsDisclosure>
    </div>
  );
}
