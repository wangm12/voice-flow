import type { WritingMode } from "../components/ContextSettings";
import type { Snippet } from "../components/SnippetsSettings";
import type { UiLanguagePreference } from "../lib/i18n";

export type Settings = {
  schema_version: number;
  api_key_configured: boolean;
  api_key_hint: string | null;
  asr_api_key_configured?: boolean;
  asr_api_key_hint?: string | null;
  asr_base_url?: string;
  asr_provider?: "groq" | "custom";
  asr_model: string;
  cleanup_model: string;
  cleanup_provider?: "groq" | "custom";
  cleanup_base_url?: string;
  cleanup_api_key_configured?: boolean;
  cleanup_api_key_hint?: string | null;
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
  onboarded: boolean;
  cleanup_enabled: boolean;
  show_tray_icon: boolean;
  hotkey: string;
  activation_mode: string;
  hotkey_error?: string | null;
  context_enabled: boolean;
  browser_access_enabled: boolean;
  context_mappings: unknown[];
  writing_modes: WritingMode[];
  snippets: Snippet[];
  output_mode: string;
  translation_target_language: string;
  selected_action_hotkey?: string;
  selected_actions_enabled?: boolean;
  dictionary_learn_enabled?: boolean;
  input_device: string;
  input_gain?: number;
};

export type SaveSettings = (
  patch: Partial<Settings>,
  options?: { persist?: boolean },
) => void;

export type AudioInputDevice = {
  name: string;
  is_default: boolean;
};
