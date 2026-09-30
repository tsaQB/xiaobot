<script setup lang="ts">
import { computed, ref } from 'vue'
import { api } from '../api/client'
import type { EngineView, Ok, SearchState, SearchTestRequest, SearchTestResult } from '../api/types'
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
import { toast } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<SearchState>('/api/search'))

function note(e: EngineView): string {
  if (e.state === 'cool') {
    const mins = Math.max(1, Math.ceil((e.cooldown_secs ?? 60) / 60))
    return L(`jeda ${mins} mnt`, `paused ${mins} min`)
  }
  if (e.state === 'off') return L('tanpa kunci', 'no key')
  if (e.id === 'wiki') return L('cadangan terakhir', 'last fallback')
  return L('siap', 'ready')
}

const legend = computed(() => (data.value ? data.value.engines.filter((e) => e.state !== 'on' || e.id === 'wiki') : []))
const anyCool = computed(() => !!data.value?.engines.some((e) => e.state === 'cool'))

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
    :text="L('Urutan mesin dipilih otomatis dari kunci yang terisi. Kunci baru langsung dipakai pencarian berikutnya.', 'The engine order follows the keys you have set. A new key is used from the next search on.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle :title="L('Rantai mesin saat ini', 'Current engine chain')" />
    <div class="card card-body">
      <div class="chain">
        <template v-for="(e, i) in data.engines" :key="e.id">
          <span v-if="i" class="arr" aria-hidden="true"><Icon name="chev" size="sm" /></span>
          <span class="eng" :class="{ off: e.state === 'off', cool: e.state === 'cool' }" :title="note(e)">
            <span class="dot" :class="{ ok: e.state === 'on', warn: e.state === 'cool' }"></span>{{ e.name }}
            <span class="sr">({{ note(e) }})</span>
          </span>
        </template>
      </div>
      <div v-if="legend.length" class="legend">
        <div v-for="e in legend" :key="e.id">
          <span class="dot" :class="{ warn: e.state === 'cool', ok: e.state === 'on' }"></span>{{ e.name }}<span class="n">{{ note(e) }}</span>
        </div>
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
