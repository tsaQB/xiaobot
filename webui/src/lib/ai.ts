import type { ProbeOutcome, RoleId, RouteRequest, RouteView } from '../api/types'
import { L } from '../i18n'

export const SPECIALISTS: readonly Exclude<RoleId, 'main'>[] = ['vision', 'video', 'audio_stt', 'image_gen', 'curator']

/** Select value of a route: "main_model", "disabled" or "m:<provider_id>:<model>". */
export function routeValue(r: RouteView): string {
  if (r.type === 'specific') return `m:${r.provider_id}:${r.model}`
  return r.type
}

/** Provider ids never contain ':', model names may, so split at the first ':' only. */
export function parseRoute(v: string): RouteRequest {
  if (v === 'main_model' || v === 'disabled') return { type: v }
  const rest = v.startsWith('m:') ? v.slice(2) : v
  const i = rest.indexOf(':')
  if (i < 0) return { type: 'main_model' }
  return { type: 'specific', provider_id: rest.slice(0, i), model: rest.slice(i + 1) }
}

export function outcomeText(o: ProbeOutcome): string {
  const t: Record<ProbeOutcome, string> = {
    supported: L('didukung', 'supported'),
    unsupported: L('tidak didukung', 'not supported'),
    inconclusive: L('belum pasti', 'inconclusive'),
    auth_failed: L('kunci ditolak', 'key refused'),
    rate_limited: L('kena batas pemakaian', 'rate limited'),
    timeout: L('waktu habis', 'timed out'),
    network_error: L('galat jaringan', 'network error'),
    protocol_mismatch: L('protokol tidak cocok', 'protocol mismatch'),
    provider_error: L('galat provider', 'provider error'),
  }
  return t[o]
}

export function isBadOutcome(o: ProbeOutcome): boolean {
  return o !== 'supported' && o !== 'unsupported' && o !== 'inconclusive'
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}
