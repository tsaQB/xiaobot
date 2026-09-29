import { onBeforeUnmount, onMounted, ref, shallowRef, watch, type Ref, type ShallowRef } from 'vue'
import { errorMessage } from '../api/client'
import { reloadTick } from '../stores/session'

export interface Loader<T> {
  data: ShallowRef<T | null>
  loading: Ref<boolean>
  error: Ref<string | null>
  /** `quiet` keeps the current data on screen (refreshes and reloads after a write). */
  reload: (quiet?: boolean) => Promise<T | null>
}

/**
 * Loads page data on mount, again after a daemon restart, and optionally
 * every `pollMs` while the tab is visible.
 */
export function useLoad<T>(fetcher: () => Promise<T>, opts: { pollMs?: number; onData?: (d: T) => void } = {}): Loader<T> {
  const data = shallowRef<T | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)
  let seq = 0

  async function reload(quiet = false): Promise<T | null> {
    const mine = ++seq
    if (!quiet || !data.value) loading.value = true
    try {
      const d = await fetcher()
      if (mine === seq) {
        data.value = d
        error.value = null
        opts.onData?.(d)
      }
      return d
    } catch (e) {
      if (mine === seq && !(quiet && data.value)) error.value = errorMessage(e)
      return null
    } finally {
      if (mine === seq) loading.value = false
    }
  }

  let timer: ReturnType<typeof setInterval> | null = null
  onMounted(() => {
    void reload()
    if (opts.pollMs) {
      timer = setInterval(() => {
        if (document.visibilityState === 'visible' && !loading.value) void reload(true)
      }, opts.pollMs)
    }
  })
  onBeforeUnmount(() => {
    seq++
    if (timer) clearInterval(timer)
  })
  watch(reloadTick, () => void reload(true))

  return { data, loading, error, reload }
}

/** Runs `fn` every `ms` while the component is mounted and the tab is visible. */
export function useInterval(fn: () => void, ms: number): { stop: () => void; start: () => void } {
  let timer: ReturnType<typeof setInterval> | null = null
  const start = (): void => {
    if (timer) return
    timer = setInterval(() => {
      if (document.visibilityState === 'visible') fn()
    }, ms)
  }
  const stop = (): void => {
    if (timer) clearInterval(timer)
    timer = null
  }
  onBeforeUnmount(stop)
  return { start, stop }
}
