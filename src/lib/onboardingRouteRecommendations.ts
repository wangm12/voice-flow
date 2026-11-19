import { canonicalLanguageCode } from "./languages";

export type OnboardingRouteRecommendationKind = "european_local" | "asia_multilingual";

export type OnboardingRouteRecommendation = {
  kind: OnboardingRouteRecommendationKind;
  systemRegion: string | null;
  selectedLanguage: string | null;
};

const EUROPEAN_REGIONS = new Set([
  "AD", "AL", "AT", "AX", "BA", "BE", "BG", "BY", "CH", "CY", "CZ", "DE", "DK", "EE", "ES",
  "FI", "FO", "FR", "GB", "GG", "GI", "GR", "HR", "HU", "IE", "IM", "IS", "IT", "JE", "LI",
  "LT", "LU", "LV", "MC", "MD", "ME", "MK", "MT", "NL", "NO", "PL", "PT", "RO", "RS", "RU",
  "SE", "SI", "SJ", "SK", "SM", "UA", "VA",
]);

const ASIAN_REGIONS = new Set([
  "AE", "AF", "AM", "AZ", "BD", "BH", "BN", "BT", "CN", "GE", "HK", "ID", "IL", "IN", "IQ",
  "IR", "JO", "JP", "KG", "KH", "KP", "KR", "KW", "KZ", "LA", "LB", "LK", "MM", "MN", "MO",
  "MV", "MY", "NP", "OM", "PH", "PK", "PS", "QA", "SA", "SG", "SY", "TH", "TJ", "TL", "TM",
  "TR", "TW", "UZ", "VN", "YE",
]);

const EUROPEAN_LANGUAGES = new Set(["de", "es", "fr", "it", "nl", "pl", "pt", "sv", "uk"]);
const ASIAN_LANGUAGES = new Set(["hi", "id", "ja", "ko", "th", "vi", "zh", "zh-TW", "yue"]);

function explicitRegion(locale: string | null | undefined): string | null {
  if (!locale) return null;
  const parts = locale.replace(/_/g, "-").split("-").slice(1);
  const region = parts.find((part) => /^[A-Za-z]{2}$/.test(part) || /^\d{3}$/.test(part));
  return region?.toUpperCase() ?? null;
}

export function recommendOnboardingRoute(
  selectedLanguage: string,
  systemLocale: string | null | undefined,
): OnboardingRouteRecommendation | null {
  if (!selectedLanguage.trim()) return null;
  const language = canonicalLanguageCode(selectedLanguage);
  if (!language) return null;
  const region = explicitRegion(systemLocale);

  if (language && EUROPEAN_LANGUAGES.has(language)) {
    return { kind: "european_local", systemRegion: region, selectedLanguage: language };
  }
  if (language && ASIAN_LANGUAGES.has(language)) {
    return { kind: "asia_multilingual", systemRegion: region, selectedLanguage: language };
  }
  // A fixed language outside the routes above must not be overridden by a
  // regional guess. Route buttons can coerce language settings, so only use
  // region alone when the user has selected automatic detection.
  if (language !== "auto") return null;
  if (region && ASIAN_REGIONS.has(region)) {
    return { kind: "asia_multilingual", systemRegion: region, selectedLanguage: language };
  }
  if (region && EUROPEAN_REGIONS.has(region)) {
    return { kind: "european_local", systemRegion: region, selectedLanguage: language };
  }
  return null;
}
