import { ref } from 'vue'
import type { Lang } from './api/types'

const STORAGE_KEY = 'xiao-lang'

/* A saved choice wins; otherwise the browser language picks Indonesian or English. */
function pickLang(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved === 'id' || saved === 'en') return saved
  } catch {
    /* storage may be blocked */
  }
  const nav = typeof navigator !== 'undefined' && navigator.language ? navigator.language : ''
  return /^(id|in)\b/i.test(nav) ? 'id' : 'en'
}

export const lang = ref<Lang>(pickLang())

/** The Indonesian or English text, whichever language is active. Reactive in render. */
export function L(id: string, en: string): string {
  return lang.value === 'en' ? en : id
}

export function locale(): string {
  return lang.value === 'en' ? 'en-US' : 'id-ID'
}

/** Number in the active locale ("1.234" or "1,234"). */
export function nf(n: number, opts?: Intl.NumberFormatOptions): string {
  return Number(n).toLocaleString(locale(), opts)
}

export function applyLang(): void {
  document.documentElement.lang = lang.value
}

export function setLang(next: Lang): void {
  lang.value = next
  try {
    localStorage.setItem(STORAGE_KEY, next)
  } catch {
    /* storage may be blocked */
  }
  applyLang()
}

export function toggleLang(): void {
  setLang(lang.value === 'en' ? 'id' : 'en')
}
