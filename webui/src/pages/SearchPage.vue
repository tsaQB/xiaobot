<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { api, seg } from '../api/client'
import type { EngineId, EngineToggleRequest, EngineView, Ok, SearchState, SearchTestRequest, SearchTestResult } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import EffectBadge from '../components/EffectBadge.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import SecretField from '../components/SecretField.vue'
import Toggle from '../components/Toggle.vue'
import { useLoad } from '../composables/useLoad'
import { fmtMs, safeHttpUrl } from '../format'
import { L } from '../i18n'
import { toast, toastError } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<SearchState>('/api/search'))

function pausedMins(e: EngineView): number {
  return Math.max(1, Math.ceil((e.cooldown_secs ?? 60) / 60))
}

/* Short state for the chain tooltip. */
function chainNote(e: EngineView): string {
  if (e.state === 'cool') return L(`jeda ${pausedMins(e)} mnt`, `paused ${pausedMins(e)} min`)
  if (e.id === 'wiki') return L('cadangan terakhir', 'last fallback')
  return L('siap', 'ready')
}

/* The row note in the engines card. */
function engineNote(e: EngineView): { text: string; kind: '' | 'warn' | 'ok' } {
  if (e.keyed && !e.available) return { text: L('Perlu kunci API', 'Needs an API key'), kind: 'warn' }
  if (e.state === 'cool') return { text: L(`Jeda ${pausedMins(e)} mnt`, `Paused ${pausedMins(e)} min`), kind: 'warn' }
  if (!enabledOf(e)) return { text: L('Dimatikan', 'Switched off'), kind: '' }
  return { text: L('Siap', 'Ready'), kind: 'ok' }
}

/* Only the engines that would be tried, in order. */
const chain = computed(() => (data.value ? data.value.engines.filter((e) => e.state === 'on' || e.state === 'cool') : []))
const anyCool = computed(() => !!data.value?.engines.some((e) => e.state === 'cool'))

/* Engine switches apply at once (not through the save bar). */
const pendingOn = reactive<Partial<Record<EngineId, boolean>>>({})
const enabledOf = (e: EngineView): boolean => pendingOn[e.id] ?? e.enabled

async function toggleEngine(e: EngineView, on: boolean): Promise<void> {
  pendingOn[e.id] = on
  try {
    const body: EngineToggleRequest = { enabled: on }
    await api.put<Ok>(`/api/search/engines/${seg(e.id)}`, body)
    if (!on) toast(L(`${e.name} dimatikan`, `${e.name} switched off`))
    else if (e.keyed && !e.available) toast(L(`${e.name} dinyalakan, tapi baru dipakai setelah kunci API diisi.`, `${e.name} switched on, but it is only used once its API key is set.`))
    else toast(L(`${e.name} dinyalakan`, `${e.name} switched on`))
    await reload(true)
  } catch (err) {
    toastError(err)
  } finally {
    delete pendingOn[e.id]
  }
}

async function resetCooldowns(): Promise<void> {
  await api.post<Ok>('/api/search/cooldowns/reset')
  toast(L('Jeda dihapus. Semua mesin dicoba lagi.', 'Pauses cleared. Every engine is tried again.'))
  await reload(true)
}

type KeyName = 'BRAVE_API_KEY' | 'TAVILY_API_KEY' | 'EXA_API_KEY'
const keyRows = computed<{ key: KeyName; name: string; url: string; note: string }[]>(() => [
  { key: 'BRAVE_API_KEY', name: 'Brave Search', url: 'https://brave.com/search/api/', note: L('Dicoba pertama.', 'Tried first.') },
  { key: 'TAVILY_API_KEY', name: 'Tavily', url: 'https://app.tavily.com/', note: L('Rekomendasi: stabil untuk riset, ada paket gratis.', 'Recommended: steady for research, with a free plan.') },
  { key: 'EXA_API_KEY', name: 'Exa', url: 'https://dashboard.exa.ai/', note: L('REST API resmi, terpisah dari Exa MCP tanpa kunci.', 'The official REST API, separate from the keyless Exa MCP.') },
])

/* Test search */
const query = ref(L('sejarah Colosseum', 'history of the Colosseum'))
const pictures = ref(false)
const result = ref<SearchTestResult | null>(null)
const asked = ref('')

async function runTest(): Promise<void> {
  const q = query.value.trim() || L('sejarah Colosseum', 'history of the Colosseum')
  const body: SearchTestRequest = { query: q, pictures: pictures.value }
  result.value = await api.post<SearchTestResult>('/api/search/test', body)
  asked.value = q
  void reload(true)
}

const images = computed(() => (result.value ? result.value.images.map((u) => safeHttpUrl(u)).filter((u): u is string => !!u).slice(0, 6) : []))
</script>

<template>
  <PageHead
    :title="L('Pencarian web', 'Web search')"
    :text="L('Mesin dicoba berurutan sampai ada yang menjawab. Nyalakan atau matikan tiap mesin di bawah; perubahan langsung dipakai pencarian berikutnya.', 'Engines are tried in order until one answers. Switch each engine on or off below; changes apply from the next search on.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle :title="L('Rantai mesin saat ini', 'Current engine chain')" />
    <div class="card card-body">
      <div v-if="chain.length" class="chain" role="list" :aria-label="L('Urutan mesin yang dicoba', 'Engines tried, in order')">
        <template v-for="(e, i) in chain" :key="e.id">
          <span v-if="i" class="arr" aria-hidden="true"><Icon name="chev" size="sm" /></span>
          <span class="eng" role="listitem" :class="{ cool: e.state === 'cool' }" :title="chainNote(e)">
            <span class="dot" :class="{ ok: e.state === 'on', warn: e.state === 'cool' }"></span>{{ e.name }}
            <span v-if="e.state === 'cool'" class="eng-n">{{ chainNote(e) }}</span>
            <span v-else class="sr">({{ chainNote(e) }})</span>
          </span>
        </template>
      </div>
      <div v-else class="note warn" role="status">
        <Icon name="alert" size="sm" />
        <div>{{ L('Tidak ada mesin pencari aktif; web_search tidak akan memberi hasil.', 'No search engine is active; web_search will return nothing.') }}</div>
      </div>
      <div class="help mt12">
        {{ L('Mesin yang baru gagal dilewati sementara: Exa MCP 10 menit setelah 429 (atau sesuai Retry-After) dan 2 menit untuk kegagalan lain; DuckDuckGo 10 menit.', 'An engine that just failed is skipped for a while: Exa MCP for 10 minutes after a 429 (or its Retry-After) and 2 minutes after other failures; DuckDuckGo for 10 minutes.') }}
      </div>
      <div class="actions mt12">
        <BusyButton class="btn sm" :run="resetCooldowns" :disabled="!anyCool" :label="L('Memproses…', 'Working…')">
          <Icon name="refresh" size="sm" />{{ L('Akhiri jeda sekarang', 'End the pauses now') }}
        </BusyButton>
      </div>
    </div>

    <SectionTitle :title="L('Mesin pencari', 'Search engines')"><EffectBadge type="live" /></SectionTitle>
    <div class="card rows">
      <div v-for="e in data.engines" :key="e.id" class="row">
        <span class="row-ic" :class="engineNote(e).kind"><Icon :name="e.keyed ? 'key' : 'globe'" /></span>
        <div class="grow">
          <div class="label">{{ e.name }}</div>
          <div class="hint eng-note"><span class="dot" :class="engineNote(e).kind" aria-hidden="true"></span>{{ engineNote(e).text }}</div>
        </div>
        <Toggle
          :model-value="enabledOf(e)"
          :disabled="e.id in pendingOn"
          :label="L(`Pakai ${e.name}`, `Use ${e.name}`)"
          @update:model-value="(v: boolean) => toggleEngine(e, v)"
        />
      </div>
    </div>
    <div class="small muted mt8">
      {{ L('Mesin berkunci yang dinyalakan tanpa kunci dilewati sampai kuncinya diisi di bawah.', 'A keyed engine that is switched on without a key is skipped until its key is set below.') }}
    </div>

    <SectionTitle :title="L('Kunci API', 'API keys')"><EffectBadge type="live" /></SectionTitle>
    <div class="card card-body">
      <div v-for="k in keyRows" :key="k.key" class="field">
        <div class="lab">{{ k.name }} <EffectBadge type="secret" /></div>
        <SecretField :secret-key="k.key" :meta="data.keys[k.key]" :locked="data.env_locks[k.key]" @changed="reload(true)" />
        <div class="help">
          <code>{{ k.key }}</code> {{ k.note }}
          <a :href="k.url" target="_blank" rel="noopener noreferrer">{{ L('Dapatkan kunci', 'Get a key') }}<Icon name="link" size="sm" /></a>
        </div>
      </div>
    </div>

    <SectionTitle :title="L('Uji pencarian', 'Test a search')" />
    <div class="card card-body">
      <form class="inline" @submit.prevent>
        <input v-model="query" class="input" enterkeyhint="search" :aria-label="L('Kata kunci', 'Search terms')" />
        <BusyButton type="submit" class="btn primary" :run="runTest" :label="L('Mencari…', 'Searching…')">
          <Icon name="search" size="sm" />{{ L('Uji', 'Test') }}
        </BusyButton>
      </form>
      <label class="flex small mt12">
        <Toggle v-model="pictures" />
        {{ L('Mode gambar (cari foto, minimal 3)', 'Picture mode (finds photos, at least 3)') }}
      </label>

      <div v-if="result" class="result" aria-live="polite">
        <div class="flex wrap">
          <Badge v-if="result.engine" kind="ok" icon="check">{{ L('Dijawab ', 'Answered by ') }}{{ result.engine }}</Badge>
          <Badge v-else kind="warn" icon="alert">{{ L('Tidak ada hasil', 'No results') }}</Badge>
          <span class="small faint">{{ fmtMs(result.ms) }}</span>
        </div>
        <p v-if="result.answer" class="para">{{ result.answer }}</p>
        <ol v-if="result.hits.length">
          <li v-for="(h, i) in result.hits" :key="i">
            <a v-if="safeHttpUrl(h.url)" :href="safeHttpUrl(h.url) ?? undefined" target="_blank" rel="noopener noreferrer">{{ h.title || h.url }}</a>
            <span v-else>{{ h.title || h.url }}</span>
            <div v-if="h.summary" class="small muted">{{ h.summary }}</div>
          </li>
        </ol>
        <div v-if="images.length" class="thumbs">
          <a v-for="(src, i) in images" :key="i" :href="src" target="_blank" rel="noopener noreferrer" :aria-label="`${L('Gambar', 'Picture')} ${i + 1}`">
            <img :src="src" alt="" loading="lazy" referrerpolicy="no-referrer" />
          </a>
        </div>
        <div class="small faint mt8">{{ L('Kueri', 'Query') }}: “{{ asked }}”</div>
        <details v-if="result.raw" class="raw">
          <summary><Icon name="chev" size="sm" />{{ L('Keluaran lengkap', 'Full output') }}</summary>
          <pre>{{ result.raw }}</pre>
        </details>
      </div>
    </div>
  </template>
</template>
