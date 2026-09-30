<script setup lang="ts">
import { computed, onMounted, ref, shallowRef } from 'vue'
import { api, errorMessage } from '../api/client'
import type { LogLine, LogsPage } from '../api/types'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import Rich from '../components/Rich'
import Seg from '../components/Seg.vue'
import { useInterval } from '../composables/useLoad'
import { fmtTime, isoDay } from '../format'
import { L } from '../i18n'
import { toast } from '../stores/ui'

const MAX_LINES = 500

/* Newest first, like the mockup. */
const lines = shallowRef<LogLine[]>([])
const last = ref(0)
const filter = ref('')
const loaded = ref(false)
const loading = ref(false)
const error = ref<string | null>(null)
const paused = ref(false)

type Level = 'all' | 'INFO' | 'WARN' | 'ERROR'
const level = ref<Level>('all')
const q = ref('')
const levels = computed<{ value: Level; label: string }[]>(() => [
  { value: 'all', label: L('Semua', 'All') },
  { value: 'INFO', label: 'INFO' },
  { value: 'WARN', label: 'WARN' },
  { value: 'ERROR', label: 'ERROR' },
])

let fetching = false
async function fetchLines(): Promise<void> {
  if (fetching) return
  fetching = true
  if (!loaded.value) loading.value = true
  try {
    const page = await api.get<LogsPage>(`/api/logs?after=${last.value}`)
    if (page.last < last.value) lines.value = [] // the daemon restarted and counts from zero again
    last.value = page.last
    filter.value = page.filter
    if (page.lines.length) lines.value = [...page.lines].reverse().concat(lines.value).slice(0, MAX_LINES)
    loaded.value = true
    error.value = null
  } catch (e) {
    if (!loaded.value) error.value = errorMessage(e)
  } finally {
    fetching = false
    loading.value = false
  }
}

const poll = useInterval(() => {
  if (!paused.value) void fetchLines()
}, 2000)
onMounted(() => {
  void fetchLines()
  poll.start()
})

const shown = computed(() => {
  const needle = q.value.trim().toLowerCase()
  return lines.value.filter(
    (l) => (level.value === 'all' || l.level === level.value) && (!needle || `${l.target} ${l.msg}`.toLowerCase().includes(needle)),
  )
})

function download(): void {
  const text = [...shown.value]
    .reverse()
    .map((l) => `${l.ts} ${l.level.padEnd(5, ' ')} ${l.target} ${l.msg}`)
    .join('\n')
  const name = `xiao-log-${isoDay()}.txt`
  const url = URL.createObjectURL(new Blob([`${text}\n`], { type: 'text/plain;charset=utf-8' }))
  const a = document.createElement('a')
  a.href = url
  a.download = name
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
  toast(L(`${name} diunduh`, `Downloaded ${name}`))
}
</script>

<template>
  <PageHead
    :title="L('Log', 'Logs')"
    :text="L('Baris terbaru dari daemon, langsung dari memori. Tidak perlu journalctl.', 'The latest daemon lines, straight from memory. No journalctl needed.')"
  />

  <LoadState v-if="!loaded" :loading="loading" :error="error" @retry="fetchLines()" />

  <template v-else>
    <div class="toolbar">
      <Seg v-model="level" :options="levels" :label="L('Level log', 'Log level')" />
      <input v-model="q" class="input" type="search" :placeholder="L('Filter teks…', 'Filter text…')" :aria-label="L('Filter teks', 'Filter text')" />
      <button type="button" class="btn" :aria-pressed="paused" @click="paused = !paused">
        <Icon :name="paused ? 'play' : 'clock'" size="sm" />{{ paused ? L('Lanjut', 'Resume') : L('Jeda', 'Pause') }}
      </button>
      <button type="button" class="btn ghost" :aria-label="L('Unduh', 'Download')" :title="L('Unduh', 'Download')" @click="download">
        <Icon name="down" size="sm" />
      </button>
    </div>
    <div class="log" role="log" aria-live="off" tabindex="0">
      <div v-for="l in shown" :key="l.seq">
        <span class="t">{{ fmtTime(l.ts) }}</span> <span :class="l.level">{{ l.level.padEnd(5, ' ') }}</span> <span class="t">{{ l.target }}</span> {{ l.msg }}
      </div>
      <div v-if="!shown.length" class="t">{{ L('Tidak ada baris yang cocok.', 'No lines match.') }}</div>
    </div>
    <div class="small muted mt8">
      <Rich
        :text="
          L(
            `Menyimpan ${MAX_LINES} baris terakhir. Level saat ini \`${filter || 'info'}\`, diatur lewat RUST_LOG di environment.`,
            `Keeps the last ${MAX_LINES} lines. The level is \`${filter || 'info'}\`, set with RUST_LOG in the environment.`,
          )
        "
      />
    </div>
  </template>
</template>
