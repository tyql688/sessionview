import i18next from "i18next";
import { initReactI18next, useTranslation } from "react-i18next";
import en from "@/i18n/en.json";
import zh from "@/i18n/zh.json";

const dictionaries = { en, zh };
type Locale = keyof typeof dictionaries;

function detectLocale(): Locale {
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

// Initialize once at module load.
i18next.use(initReactI18next).init({
  resources: {
    en: { translation: dictionaries.en },
    zh: { translation: dictionaries.zh },
  },
  lng: detectLocale(),
  fallbackLng: "en",
  interpolation: {
    escapeValue: false,
    // en.json / zh.json use single-brace placeholders ({count}), not the
    // i18next default double-brace ({{count}}).
    prefix: "{",
    suffix: "}",
  },
  returnNull: false,
});

// Imperative current-locale read for non-React callers (e.g. lib/formatters.ts).
// i18next is a global singleton; normalize its language tag to our Locale union.
export function getLocale(): Locale {
  return i18next.language?.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function useI18n() {
  const { t, i18n } = useTranslation();
  return {
    t: (key: string, options?: Record<string, unknown>): string => t(key, options ?? {}),
    locale: i18n.language as Locale,
    setLocale: (next: Locale) => i18n.changeLanguage(next),
  };
}

export { i18next };
export type { Locale };
