import { ref } from 'vue'
import { L, lang, nf } from './i18n'

/** Wall clock in ms, ticking once a second so relative times and the uptime pill stay fresh. */
export const now = ref(Date.now())
setInterval(() => {
  now.value = Date.now()
}, 1000)

const MONTHS_ID = ['Jan', 'Feb', 'Mar', 'Apr', 'Mei', 'Jun', 'Jul', 'Agu', 'Sep', 'Okt', 'Nov', 'Des']
const MONTHS_EN = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

const pad = (n: number): string => String(n).padStart(2, '0')

function parse(iso: string | null | undefined): Date | null {
  if (!iso) return null
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? null : d
}

function month(d: Date): string {
  return (lang.value === 'en' ? MONTHS_EN : MONTHS_ID)[d.getMonth()] ?? ''
}

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

/** "hari ini 09:52", "kemarin 21:14", "29 Sep 09:41" or "29 Sep 2025 09:41". */
export function fmtWhen(iso: string | null | undefined): string {
  const d = parse(iso)
  if (!d) return '—'
  const today = new Date(now.value)
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`
  if (sameDay(d, today)) return `${L('hari ini', 'today')} ${hm}`
  if (sameDay(d, yesterday)) return `${L('kemarin', 'yesterday')} ${hm}`
  const year = d.getFullYear() === today.getFullYear() ? '' : ` ${d.getFullYear()}`
  return `${d.getDate()} ${month(d)}${year} ${hm}`
}

/** "12 Sep", or "12 Sep 2025" outside the current year. */
export function fmtDay(iso: string | null | undefined): string {
  const d = parse(iso)
  if (!d) return '—'
  const year = d.getFullYear() === new Date(now.value).getFullYear() ? '' : ` ${d.getFullYear()}`
  return `${d.getDate()} ${month(d)}${year}`
}

/** "09:57:05". */
export function fmtTime(iso: string | null | undefined): string {
  const d = parse(iso)
  if (!d) return '--:--:--'
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

/** "YYYY-MM-DD" of today, for file names. */
export function isoDay(d = new Date()): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** A running time: "2 dtk", "7 mnt", "2j 14m", "3h 4j" (English: "2 s", "7 min", "2h 14m", "3d 4h"). */
export function fmtDuration(totalSecs: number): string {
  const s = Math.max(0, Math.floor(totalSecs))
  if (s < 60) return L(`${s} dtk`, `${s} s`)
  const m = Math.floor(s / 60)
  if (m < 60) return L(`${m} mnt`, `${m} min`)
  const h = Math.floor(m / 60)
  if (h < 24) return L(`${h}j ${m % 60}m`, `${h}h ${m % 60}m`)
  const d = Math.floor(h / 24)
  return L(`${d}h ${h % 24}j`, `${d}d ${h % 24}h`)
}

/** Seconds with one decimal from milliseconds: "2,1 dtk" / "2.1 s". */
export function fmtMs(ms: number): string {
  const secs = nf(Math.round(ms / 100) / 10, { maximumFractionDigits: 1 })
  return L(`${secs} dtk`, `${secs} s`)
}

/** "N dtk lalu" / "N s ago", scaling up to minutes, hours and days. */
export function fmtAgoSecs(secs: number): string {
  const s = Math.max(0, Math.floor(secs))
  if (s < 1) return L('baru saja', 'just now')
  if (s < 60) return L(`${s} dtk lalu`, `${s} s ago`)
  const m = Math.floor(s / 60)
  if (m < 60) return L(`${m} mnt lalu`, `${m} min ago`)
  const h = Math.floor(m / 60)
  if (h < 24) return L(`${h} jam lalu`, `${h} h ago`)
  const d = Math.floor(h / 24)
  return L(`${d} hari lalu`, d === 1 ? '1 day ago' : `${d} days ago`)
}

export function fmtAgo(iso: string | null | undefined): string {
  const d = parse(iso)
  if (!d) return '—'
  return fmtAgoSecs((now.value - d.getTime()) / 1000)
}

/** "dalam 6 hari" / "in 6 days". */
export function fmtUntil(iso: string | null | undefined): string {
  const d = parse(iso)
  if (!d) return '—'
  const s = Math.max(0, Math.floor((d.getTime() - now.value) / 1000))
  const m = Math.floor(s / 60)
  const h = Math.floor(m / 60)
  const days = Math.floor(h / 24)
  if (days >= 1) return L(`dalam ${days} hari`, days === 1 ? 'in 1 day' : `in ${days} days`)
  if (h >= 1) return L(`dalam ${h} jam`, h === 1 ? 'in 1 hour' : `in ${h} hours`)
  return L(`dalam ${Math.max(1, m)} menit`, `in ${Math.max(1, m)} min`)
}

/** "412 KB", "1,7 MB" (locale decimals). */
export function fmtBytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return '—'
  if (n < 1024) return `${nf(n)} B`
  if (n < 1024 * 1024) return `${nf(Math.round(n / 1024))} KB`
  if (n < 1024 * 1024 * 1024) return `${nf(Math.round((n / 1048576) * 10) / 10, { maximumFractionDigits: 1 })} MB`
  return `${nf(Math.round((n / 1073741824) * 10) / 10, { maximumFractionDigits: 1 })} GB`
}

/** "A, B dan C" / "A, B and C". */
export function listJoin(items: string[]): string {
  if (items.length <= 1) return items.join('')
  const head = items.slice(0, -1).join(', ')
  return `${head} ${L('dan', 'and')} ${items[items.length - 1] ?? ''}`
}

/** Only http(s) links leave the console. */
export function safeHttpUrl(url: string | null | undefined): string | null {
  if (!url) return null
  return /^https?:\/\//i.test(url.trim()) ? url.trim() : null
}
