import type { ActivationMode } from "../lib/activationCopy";
import type { ContextMapping, WritingMode } from "../components/ContextSettings";
import type { Snippet } from "../components/SnippetsSettings";
import type { UiLanguagePreference } from "../lib/i18n";
import type { ProviderId } from "../lib/providers";

export type ProviderKeyView = {
  configured: boolean;
  hint?: string | null;
};

export type Settings = {
  schema_version: number;
  api_key_configured: boolean;
  api_key_hint: string | null;
  asr_api_key_configured?: boolean;
  asr_api_key_hint?: string | null;
  asr_base_url?: string;
  asr_provider?: ProviderId;
  asr_model: string;
  cleanup_model: string;
  cleanup_provider?: ProviderId;
  cleanup_base_url?: string;
  cleanup_api_key_configured?: boolean;
  cleanup_api_key_hint?: string | null;
  custom_base_url?: string;
  custom_asr?: boolean;
  custom_llm?: boolean;
  ollama_base_url?: string;
  local_whisper_base_url?: string;
  provider_keys?: Partial<Record<ProviderId, ProviderKeyView>>;
  language: string;
  ui_language: UiLanguagePreference;
  theme: "system" | "light" | "dark";
  dictionary: string[];
  chunk_threshold_secs: number;
  chunk_length_secs: number;
  long_output_mode: string;
  delivery_policy: string;
  keep_audio_days: number;
  keep_history_days: number;
  keep_success_audio?: boolean;
  onboarded: boolean;
  cleanup_enabled: boolean;
  strict_offline_enabled?: boolean;
  cleanup_intensity?: "auto" | "off" | "light" | "standard" | "heavy";
  accurate_asr_provider?: ProviderId;
  accurate_asr_model?: string;
  accurate_asr_base_url?: string;
  cascade_timeout_ms?: number;
  cascade_proper_noun_threshold?: number;
  window_ocr_enabled?: boolean;
  screen_action_hotkey?: string;
  vision_provider?: string;
  vision_model?: string;
  show_tray_icon: boolean;
  hotkey: string;
  activation_mode: ActivationMode;
  hotkey_error?: string | null;
  context_enabled: boolean;
  browser_access_enabled: boolean;
  context_mappings: ContextMapping[];
  writing_modes: WritingMode[];
  snippets: Snippet[];
  output_mode: string;
  translation_target_language: string;
  selected_action_hotkey?: string;
  selected_actions_enabled?: boolean;
  dictionary_learn_enabled?: boolean;
  input_device: string;
  input_gain?: number;
  verbatim_hotkey?: string;
  translation_hotkey?: string;
  extra_recording_buffer_ms?: number;
  audio_feedback_enabled?: boolean;
  audio_feedback_volume?: number;
  vad_enabled?: boolean;
  always_on_microphone?: boolean;
  clamshell_microphone?: string;
  autostart_enabled?: boolean;
  whats_new_last_seen_version?: string;
  debug_mode?: boolean;
  fuzzy_dictionary_enabled?: boolean;

};

export type OnDeviceModelStatus = {
  id: string;
  label: string;
  bytes: number;
  sha256: string | null;
  state: string;
  inference_ready: boolean;
  downloaded_bytes: number;
  error: string | null;
  revision: string | null;
  file_count: number;
  platform_supported: boolean;
  runtime_status: string;
  loaded: boolean;
};

export type LocalCleanupStatus = {
  available: boolean;
  status: string;
  message: string;
  model: string;
};

export type SaveSettings = (
  patch: Partial<Settings>,
  options?: { persist?: boolean },
) => void;

export type AudioInputDevice = {
  name: string;
  is_default: boolean;
};
