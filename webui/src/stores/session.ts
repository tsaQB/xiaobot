/*
 * App-wide state: who is signed in, the daemon summary the shell shows on
 * every page (uptime pill, failed-queue badge), and the restart flow.
 */
import { computed, reactive, ref } from 'vue'
import { api, ApiError } from '../api/client'
import type { AuthState, Overview } from '../api/types'
import { now } from '../format'
import { L } from '../i18n'
import { abortStreams } from './chat'
import { toast } from './ui'

/* ---------- Auth ---------- */

export const auth = reactive<{ state: AuthState | null; loaded: boolean; error: string | null }>({
  state: null,
  loaded: false,
  error: null,
})

export async function loadAuth(): Promise<AuthState | null> {
  try {
    auth.state = await api.get<AuthState>('/api/auth/state')
    auth.error = null
  } catch (e) {
    auth.error = e instanceof Error ? e.message : String(e)
  }
  auth.loaded = true
  return auth.state
}

export function markSignedOut(): void {
  abortStreams()
  if (auth.state) auth.state = { ...auth.state, authenticated: false, owner_id: null, session_expires: null }
}

/* ---------- Shell summary ---------- */

export const shell = reactive({
  uptimeSecs: null as number | null,
  uptimeAt: 0,
  failed: 0,
  overviewAt: 0,
  botUsername: null as string | null,
  version: '' as string,
})

export const uptimeNow = computed(() => {
  if (shell.uptimeSecs === null) return null
  return shell.uptimeSecs + Math.max(0, (now.value - shell.uptimeAt) / 1000)
})

export function applyOverview(o: Overview): void {
  shell.uptimeSecs = o.system.uptime_secs
  shell.uptimeAt = Date.now()
  shell.failed = o.queue.failed
  shell.overviewAt = Date.now()
  shell.version = o.system.version
  if (o.telegram.username) shell.botUsername = o.telegram.username
  restart.needed = o.restart_needed
}

export async function refreshShell(force = false): Promise<void> {
  if (!force && Date.now() - shell.overviewAt < 25_000) return
  try {
    applyOverview(await api.get<Overview>('/api/overview'))
  } catch {
    /* the pages report their own errors */
  }
}

/* ---------- Restart ---------- */

export const restart = reactive({ needed: false, running: false })

/** Bumped after the daemon comes back so every page reloads its data. */
export const reloadTick = ref(0)

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))

async function answers(): Promise<boolean> {
  try {
    const res = await fetch('/api/auth/state', { credentials: 'same-origin', cache: 'no-store' })
    return res.ok
  } catch {
    return false
  }
}

/** POST /api/system/restart, then wait for the daemon in the background (overlay shown meanwhile). */
export async function restartDaemon(): Promise<void> {
  try {
    await api.post('/api/system/restart')
  } catch (e) {
    if (!(e instanceof ApiError && e.code === 'network')) throw e
  }
  void waitForRestart()
}

/**
 * Polls /api/auth/state every 1.5 s: first until the old process stops
 * answering (at most ~15 s), then until the new one does.
 */
async function waitForRestart(): Promise<void> {
  restart.running = true
  const started = Date.now()
  let wentDown = false
  while (!wentDown && Date.now() - started < 15_000) {
    await sleep(1500)
    wentDown = !(await answers())
  }
  let back = !wentDown
  while (!back && Date.now() - started < 180_000) {
    await sleep(1500)
    back = await answers()
  }
  restart.running = false
  if (!back) {
    toast(L('Daemon belum kembali. Periksa layanan di server.', 'The daemon has not come back. Check the service on the server.'), true)
    return
  }
  restart.needed = false
  await loadAuth()
  await refreshShell(true)
  reloadTick.value++
  toast(L('Daemon kembali online.', 'The daemon is back online.'))
}
