import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';

import en from '@/locales/en.json';
import zhCN from '@/locales/zh-CN.json';

export const resources = {
  en: { translation: en },
  'zh-CN': { translation: zhCN },
} as const;

export const supportedLanguages = [
  { code: 'en', name: 'English' },
  { code: 'zh-CN', name: '简体中文' },
] as const;

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: 'en',
    interpolation: {
      escapeValue: false, // React already handles XSS
    },
    detection: {
      order: ['localStorage', 'navigator'],
      lookupLocalStorage: 'aleph-language',
      caches: ['localStorage'],
    },
  });

// Function to change language programmatically
export const changeLanguage = (lng: string) => {
  if (lng === 'system') {
    // Use browser language detection
    const browserLang = navigator.language;
    const matchedLang = supportedLanguages.find(
      (l) => browserLang.startsWith(l.code.split('-')[0])
    );
    i18n.changeLanguage(matchedLang?.code || 'en');
    localStorage.removeItem('aleph-language');
  } else {
    i18n.changeLanguage(lng);
    localStorage.setItem('aleph-language', lng);
  }
};

// Get current language
export const getCurrentLanguage = () => i18n.language;

export default i18n;
