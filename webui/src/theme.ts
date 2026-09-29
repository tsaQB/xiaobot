import { ref } from 'vue'
import { L } from './i18n'

export type Theme = 'auto' | 'light' | 'dark'

const STORAGE_KEY = 'xiao-theme'

function savedTheme(): Theme {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved === 'auto' || saved === 'light' || saved === 'dark') return saved
  } catch {
    /* storage may be blocked */
  }
  return 'auto'
}

export const theme = ref<Theme>(savedTheme())

/** "auto" follows the system; the other two pin `data-theme` on <html>. */
export function applyTheme(): void {
  const root = document.documentElement
  if (theme.value === 'auto') root.removeAttribute('data-theme')
  else root.setAttribute('data-theme', theme.value)
}

export function cycleTheme(): void {
  const next: Record<Theme, Theme> = { auto: 'light', light: 'dark', dark: 'auto' }
  theme.value = next[theme.value]
  try {
    localStorage.setItem(STORAGE_KEY, theme.value)
  } catch {
    /* storage may be blocked */
  }
  applyTheme()
}

export function themeName(t: Theme): string {
  return { auto: L('otomatis', 'auto'), light: L('terang', 'light'), dark: L('gelap', 'dark') }[t]
}

export function themeIcon(t: Theme): string {
  return { auto: 'auto', light: 'sun', dark: 'moon' }[t]
}
