<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api, ApiError } from '../api/client'
import type { McpState, McpTestResult, Ok } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import Rich from '../components/Rich'
import SectionTitle from '../components/SectionTitle.vue'
import { useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { L, lang, nf } from '../i18n'
import { confirmAction, toast, useSaveBar } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<McpState>('/api/mcp'))
const { form, dirty, reset } = useForm({ url: '' })
const invalid = ref<string | null>(null)
const test = ref<McpTestResult | null>(null)

watch(data, (d) => {
  if (d && !dirty.value) reset({ url: d.url })
})
watch(
  () => form.url,
  () => {
    invalid.value = null
  },
)

const locked = computed(() => data.value?.env_locks.EXA_MCP_URL ?? null)

async function save(): Promise<void> {
  const url = form.url.trim()
  try {
    await api.put<Ok>('/api/mcp', { url })
  } catch (e) {
    if (e instanceof ApiError && e.code === 'invalid') {
      invalid.value = e.message
      throw new Error(L('Alamat ditolak firewall SSRF.', 'The SSRF firewall refused the address.'))
    }
    throw e
  }
  toast(L('Disimpan dan langsung dipakai.', 'Saved and in use now.'))
  const d = await reload(true)
  if (d) reset({ url: d.url })
}

useSaveBar({ dirty, save, discard: () => data.value && reset({ url: data.value.url }) })

async function runTest(): Promise<void> {
  test.value = await api.post<McpTestResult>('/api/mcp/test', { url: form.url.trim() })
}

function restoreDefault(): void {
  const def = data.value?.default_url ?? ''
  confirmAction({
    title: L('Pulihkan endpoint default?', 'Restore the default endpoint?'),
    text: L(`Endpoint MCP kembali ke \`${def}\`.`, `The MCP endpoint goes back to \`${def}\`.`),
    label: L('Pulihkan', 'Restore'),
    danger: false,
    run: async () => {
      await api.post<Ok>('/api/mcp/reset')
      toast(L('Endpoint default dipulihkan', 'Default endpoint restored'))
      test.value = null
      const d = await reload(true)
      if (d) reset({ url: d.url })
    },
  })
}

/* Localized descriptions for the tools the mockup lists; others use the server text. */
function toolDesc(name: string, fallback: string): string {
  const t: Record<string, [string, string]> = {
    web_search: ['Cari di web lewat rantai mesin pencari.', 'Searches the web through the engine chain.'],
    fetch_url: ['Ambil dan baca isi halaman (aman SSRF).', 'Fetches and reads a page (SSRF-safe).'],
    send_photo: ['Kirim satu foto dari URL publik.', 'Sends one photo from a public URL.'],
    send_collage: ['Album 2–10 foto (sendMediaGroup).', 'An album of 2–10 photos (sendMediaGroup).'],
    send_slideshow: ['Tayangan slide foto yang bisa digeser.', 'A photo slideshow you can swipe.'],
    send_audio: ['Kirim berkas audio musik.', 'Sends a music file.'],
    send_voice: ['Kirim voice note dengan waveform.', 'Sends a voice note with a waveform.'],
    send_location: ['Kirim pin lokasi.', 'Sends a location pin.'],
    send_document: ['Kirim dokumen dari URL.', 'Sends a document from a URL.'],
    create_document: ['Buat PDF, HTML, kode, CSV/JSON, SVG lalu kirim.', 'Creates a PDF, HTML, code, CSV/JSON or SVG file and sends it.'],
    create_archive: ['Paketkan banyak berkas jadi satu ZIP.', 'Packs several files into one ZIP.'],
    create_quiz: ['Kuis native: gambar, penjelasan, batas waktu.', 'Native quizzes with pictures, explanations and a time limit.'],
    send_live_photo: ['Live photo dari foto + MP4 ≤10 dtk.', 'A live photo from a picture and an MP4 of 10 s or less.'],
  }
  const d = t[name]
  return d ? L(d[0], d[1]) : fallback
}
</script>

<template>
  <PageHead title="MCP">
    {{ L('Xiao memakai satu endpoint MCP untuk pencarian tanpa kunci (Exa MCP). Tool bawaan di bawah selalu tersedia untuk model.', 'Xiao uses one MCP endpoint for keyless search (Exa MCP). The built-in tools below are always available to the model.') }}
  </PageHead>

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle title="Endpoint"><EffectBadge type="live" /></SectionTitle>
    <div class="card card-body">
      <label class="field mt0">
        <span class="lab">{{ L('URL endpoint', 'Endpoint URL') }}</span>
        <input v-model="form.url" class="input mono" inputmode="url" spellcheck="false" autocomplete="off" :disabled="!!locked" :aria-invalid="!!invalid" />
        <span v-if="invalid" class="field-err">{{ invalid }}</span>
        <span class="help">
          <code>EXA_MCP_URL</code>
          {{ L('diperiksa firewall SSRF sebelum disimpan: alamat loopback, privat, dan link-local ditolak.', 'is checked by the SSRF firewall before it is saved: loopback, private and link-local addresses are refused.') }}
        </span>
      </label>
      <EnvLock :src="locked" />
      <div class="actions mt12">
        <BusyButton class="btn" :run="runTest" :label="L('Menghubungi…', 'Connecting…')"><Icon name="play" size="sm" />{{ L('Uji koneksi', 'Test connection') }}</BusyButton>
        <button type="button" class="btn ghost" :disabled="!!locked || data.url === data.default_url" @click="restoreDefault">
          <Icon name="refresh" size="sm" />{{ L('Pulihkan default', 'Restore default') }}
        </button>
      </div>
      <template v-if="test">
        <div v-if="test.ok" class="note ok mt12">
          <Icon name="check" size="sm" />
          <div>
            <template v-if="lang === 'en'">Connected to <b>{{ test.host ?? '—' }}</b> in {{ nf(test.ms) }} ms.</template>
            <template v-else>Terhubung ke <b>{{ test.host ?? '—' }}</b> dalam {{ nf(test.ms) }} ms.</template>
            <div v-if="test.snippet" class="small muted mt8 break">{{ test.snippet }}</div>
          </div>
        </div>
        <div v-else class="note err mt12">
          <Icon name="alert" size="sm" /><div>{{ test.error ?? L('Endpoint tidak menjawab.', 'The endpoint did not answer.') }}</div>
        </div>
      </template>
    </div>

    <SectionTitle :title="L('Tool bawaan', 'Built-in tools')">
      <Badge>{{ nf(data.tools.length) }}{{ L(' tool', data.tools.length === 1 ? ' tool' : ' tools') }}</Badge>
    </SectionTitle>
    <div class="card rows">
      <div v-for="t in data.tools" :key="t.name" class="row">
        <div class="grow">
          <div class="label mono">{{ t.name }}</div>
          <div class="hint">{{ toolDesc(t.name, t.description) }}</div>
        </div>
        <Badge v-if="t.guest" kind="info">{{ L('Juga guest dan inline', 'Also guest and inline') }}</Badge>
      </div>
      <div v-if="!data.tools.length" class="empty">{{ L('Tidak ada tool.', 'No tools.') }}</div>
    </div>
    <div class="small muted mt8">
      <Rich
        :text="L('Mode guest dan inline hanya boleh memakai tool baca: web_search dan fetch_url. Chat di WebUI memakai semua tool.', 'Guest and inline mode may only use the read tools: web_search and fetch_url. Chat in the WebUI uses every tool.')"
      />
    </div>
  </template>
</template>
