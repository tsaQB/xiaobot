import type { WaPhase } from '../api/types'
import { L } from '../i18n'

export type StatusKind = 'ok' | 'info' | 'warn' | 'err' | ''

export interface TelegramStatus {
  kind: StatusKind
  /** Short label for a badge or a tile. */
  text: string
  /** One sentence for the "not running yet" case. */
  hint: string | null
}

/**
 * The Telegram badge, shared by the Telegram page and Home:
 * running and polled → Online; running but no poll yet → Connecting;
 * a valid bot that is not running → waiting for the token and owner; otherwise Offline.
 */
export function telegramStatus(running: boolean, online: boolean, hasBot: boolean): TelegramStatus {
  if (running && online) return { kind: 'ok', text: 'Online', hint: null }
  if (running) return { kind: 'info', text: L('Menyambung…', 'Connecting…'), hint: null }
  if (hasBot) {
    return {
      kind: 'warn',
      text: L('Token valid, belum berjalan', 'Token valid, not running yet'),
      hint: L(
        'Telegram menyala sendiri beberapa detik setelah token dan owner terisi.',
        'Telegram starts by itself a few seconds after the token and owner are set.',
      ),
    }
  }
  return { kind: 'err', text: 'Offline', hint: null }
}

export function waPhaseText(p: WaPhase): string {
  const t: Record<WaPhase, string> = {
    off: L('Gateway nonaktif', 'Gateway off'),
    starting: L('Memulai…', 'Starting…'),
    pairing: L('Menunggu penautan', 'Waiting to be linked'),
    online: 'Online',
    retrying: L('Mencoba ulang', 'Retrying'),
    logged_out: L('Keluar dari WhatsApp', 'Logged out of WhatsApp'),
  }
  return t[p]
}

export function waPhaseKind(p: WaPhase, linked: boolean): StatusKind {
  if (p === 'online' && linked) return 'ok'
  if (p === 'retrying') return 'warn'
  if (p === 'logged_out') return 'err'
  if (p === 'pairing' || p === 'starting') return 'info'
  return ''
}

/** "m:ss" for a countdown. */
export function clock(secs: number): string {
  const s = Math.max(0, Math.ceil(secs))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}
