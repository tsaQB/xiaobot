<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api, errorMessage, seg } from '../api/client'
import type { AiState, Ok, ProbeResult, ProviderView, RoleId, RoleView, SettingKey, SettingsRequest, WriteResult } from '../api/types'
import CapsSheet from '../components/ai/CapsSheet.vue'
import ProviderCard from '../components/ai/ProviderCard.vue'
import ProviderSheet from '../components/ai/ProviderSheet.vue'
import RoleCard from '../components/ai/RoleCard.vue'
import BusyButton from '../components/BusyButton.vue'
import CapChips from '../components/CapChips.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import NumField from '../components/NumField.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import { intIn, useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { L } from '../i18n'
import { isBadOutcome, outcomeText, parseRoute, routeValue, SPECIALISTS } from '../lib/ai'
import { ROLE_CAPS, roleMeta } from '../lib/caps'
import { toast, useSaveBar, useSheet } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<AiState>('/api/ai'))

interface AiForm {
  provider: string
  model: string
  routes: Record<string, string>
  fallback: string
  gen: string
  conn: string
  dl: string
  aiConn: string
}

const empty: AiForm = { provider: '', model: '', routes: {}, fallback: 'none', gen: '', conn: '', dl: '', aiConn: '' }
const { form, dirty, reset, initial } = useForm<AiForm>(empty)

function fromState(d: AiState): AiForm {
  const main = d.roles.find((r) => r.id === 'main')
  const active = d.providers.find((p) => p.active) ?? d.providers[0]
  let provider = active?.id ?? ''
  let model = active?.active_model ?? ''
  if (main && main.route.type === 'specific') {
    provider = main.route.provider_id
    model = main.route.model
  }
  const routes: Record<string, string> = {}
  for (const r of d.roles) if (r.id !== 'main') routes[r.id] = routeValue(r.route)
  return {
    provider,
    model,
    routes,
    fallback: d.image.fallback,
    gen: String(d.image.gen_timeout),
    conn: String(d.image.connect_timeout),
    dl: String(d.image.download_timeout),
    aiConn: String(d.ai_limits.connect_timeout),
  }
}

watch(data, (d) => {
  if (d && !dirty.value) reset(fromState(d))
})

const mainRole = computed(() => data.value?.roles.find((r) => r.id === 'main') ?? null)
const specialists = computed(() => (data.value ? data.value.roles.filter((r) => r.id !== 'main') : []))
const chosenProvider = computed<ProviderView | null>(() => data.value?.providers.find((p) => p.id === form.provider) ?? null)
const modelOptions = computed(() => {
  const list = chosenProvider.value ? [...chosenProvider.value.models] : []
  if (form.model && !list.includes(form.model)) list.unshift(form.model)
  return list
})
const locks = computed(() => data.value?.env_locks ?? {})

function onProvider(): void {
  const p = chosenProvider.value
  form.model = p ? p.active_model || p.models[0] || '' : ''
}

const numInvalid = (v: string): string | null => (v.trim() === '' || intIn(v, 1, 600) !== null ? null : L('Bilangan bulat 1 sampai 600.', 'A whole number from 1 to 600.'))

/* The one save bar sends only what changed: the main model, routes, then settings. */
async function save(): Promise<void> {
  const was = initial()
  const nums: [SettingKey, string, string][] = [
    ['IMAGE_GENERATION_TIMEOUT_SECS', form.gen, was.gen],
    ['IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS', form.conn, was.conn],
    ['IMAGE_DOWNLOAD_TIMEOUT_SECS', form.dl, was.dl],
    ['AI_PROVIDER_CONNECT_TIMEOUT_SECS', form.aiConn, was.aiConn],
  ]
  const settings: SettingsRequest = {}
  for (const [key, now, before] of nums) {
    if (now === before) continue
    const n = intIn(now, 1, 600)
    if (n === null) throw new Error(L(`${key} harus bilangan bulat 1 sampai 600.`, `${key} must be a whole number from 1 to 600.`))
    settings[key] = String(n)
  }
  if (form.fallback !== was.fallback) settings.IMAGE_FALLBACK_PROVIDER = form.fallback

  if (form.provider !== was.provider || form.model !== was.model) {
    if (!form.provider || !form.model) throw new Error(L('Pilih provider dan model dulu.', 'Pick a provider and a model first.'))
    await api.post<Ok>('/api/ai/active', { provider_id: form.provider, model: form.model })
  }
  for (const role of SPECIALISTS) {
    const v = form.routes[role]
    if (v !== undefined && v !== was.routes[role]) await api.put<Ok>(`/api/ai/routes/${seg(role)}`, parseRoute(v))
  }
  if (Object.keys(settings).length) await api.put<WriteResult>('/api/settings', settings)

  toast(
    settings.AI_PROVIDER_CONNECT_TIMEOUT_SECS !== undefined
      ? L('Disimpan. Batas waktu provider berlaku setelah restart.', 'Saved. The provider timeout applies after a restart.')
      : L('Disimpan. Daemon langsung memakai model baru.', 'Saved. The daemon uses the new model right away.'),
  )
  const d = await reload(true)
  if (d) reset(fromState(d))
}

useSaveBar({
  dirty,
  save,
  discard: () => {
    if (data.value) reset(fromState(data.value))
  },
})

/* Tests */
async function probeMain(): Promise<void> {
  const r = await api.post<ProbeResult>('/api/ai/probe/main')
  toast(L(`Main: pengujian selesai (${outcomeText(r.outcome)})`, `Main: test finished (${outcomeText(r.outcome)})`), isBadOutcome(r.outcome))
  await reload(true)
}

async function probeAll(): Promise<void> {
  const roles = specialists.value.filter((r) => r.route.type !== 'disabled')
  const problems: string[] = []
  let tested = 0
  for (const r of roles) {
    const name = roleMeta(r.id).name
    try {
      const res = await api.post<ProbeResult>(`/api/ai/probe/${seg(r.id)}`)
      tested++
      if (isBadOutcome(res.outcome)) problems.push(`${name} (${outcomeText(res.outcome)})`)
    } catch (e) {
      problems.push(`${name} (${errorMessage(e)})`)
    }
  }
  const head = L(`${tested} spesialis diuji.`, tested === 1 ? '1 specialist tested.' : `${tested} specialists tested.`)
  toast(problems.length ? `${head} ${L('Bermasalah', 'Problems')}: ${problems.join(', ')}.` : head, problems.length > 0)
  await reload(true)
}

/* Sheets */
const capsSheet = useSheet()
const capsRole = ref<RoleView | null>(null)
function openCaps(role: RoleView | null): void {
  if (!role) return
  capsRole.value = role
  capsSheet.show()
}

const providerSheet = useSheet()
const editing = ref<ProviderView | null>(null)
function openProvider(p: ProviderView | null): void {
  editing.value = p
  providerSheet.show()
}

function roleById(id: RoleId): RoleView | null {
  return data.value?.roles.find((r) => r.id === id) ?? null
}
</script>

<template>
  <PageHead
    :title="L('AI dan model', 'AI and models')"
    :text="L('Provider, model utama, dan model spesialis. Perubahan langsung dipakai daemon tanpa restart.', 'Providers, the main model and specialist models. The daemon uses changes right away, without a restart.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle :title="L('Model utama', 'Main model')"><EffectBadge type="live" /></SectionTitle>
    <div class="card card-body">
      <div v-if="!data.providers.length" class="note warn">
        <Icon name="alert" size="sm" />
        <div>{{ L('Belum ada provider. Tambahkan satu di bawah agar Xiao bisa menjawab.', 'No provider yet. Add one below so Xiao can answer.') }}</div>
      </div>
      <template v-else>
        <div class="grid two">
          <label class="field mt0">
            <span class="lab">Provider</span>
            <select v-model="form.provider" class="select" @change="onProvider">
              <option v-for="p in data.providers" :key="p.id" :value="p.id">{{ p.name }} ({{ p.endpoint }})</option>
            </select>
          </label>
          <label class="field mt0">
            <span class="lab">Model</span>
            <select v-model="form.model" class="select">
              <option v-for="m in modelOptions" :key="m" :value="m">{{ m }}</option>
            </select>
          </label>
        </div>
        <div class="help">
          {{ L('Dipakai untuk percakapan, tool, dan riwayat. Spesialis di bawah hanya menerima media dan pertanyaan saat itu.', 'Used for conversations, tools and history. The specialists below only receive the media and the question at hand.') }}
        </div>
        <div v-if="mainRole?.error" class="help err-text">{{ mainRole.error }}</div>
        <CapChips v-if="mainRole" class="mt12" :caps="mainRole.caps" :keys="ROLE_CAPS.main" />
        <div class="actions mt12">
          <BusyButton class="btn" :run="probeMain" :label="L('Menguji…', 'Testing…')"><Icon name="play" size="sm" />{{ L('Uji model', 'Test model') }}</BusyButton>
          <button type="button" class="btn ghost" @click="openCaps(roleById('main'))"><Icon name="edit" size="sm" />{{ L('Koreksi kemampuan', 'Correct capabilities') }}</button>
        </div>
      </template>
    </div>

    <SectionTitle title="Provider">
      <button type="button" class="btn sm" @click="openProvider(null)"><Icon name="plus" size="sm" />{{ L('Tambah', 'Add') }}</button>
    </SectionTitle>
    <div class="stack">
      <ProviderCard
        v-for="p in data.providers"
        :key="p.id"
        :provider="p"
        :locked="locks[`provider:${p.id}`]"
        @edit="openProvider(p)"
        @changed="reload(true)"
      />
      <div v-if="!data.providers.length" class="card"><div class="empty">{{ L('Belum ada provider.', 'No providers yet.') }}</div></div>
    </div>

    <SectionTitle :title="L('Model spesialis', 'Specialist models')">
      <BusyButton class="btn sm" :run="probeAll" :label="L('Menguji 5 model…', 'Testing 5 models…')"><Icon name="play" size="sm" />{{ L('Uji semua', 'Test all') }}</BusyButton>
    </SectionTitle>
    <div class="grid two">
      <template v-for="r in specialists" :key="r.id">
        <RoleCard
          v-if="form.routes[r.id] !== undefined"
          v-model="form.routes[r.id]!"
          :role="r"
          :providers="data.providers"
          @changed="reload(true)"
          @override="openCaps(r)"
        />
      </template>
    </div>

    <SectionTitle :title="L('Pembuatan gambar', 'Image generation')" />
    <div class="card card-body">
      <label class="field mt0">
        <span class="lab">{{ L('Cadangan bila provider gagal', 'Fallback when the provider fails') }} <EffectBadge type="live" /></span>
        <select v-model="form.fallback" class="select" :disabled="!!locks.IMAGE_FALLBACK_PROVIDER">
          <option value="none">{{ L('Tidak ada', 'None') }}</option>
          <option value="pollinations">{{ L('Pollinations (model flux)', 'Pollinations (flux model)') }}</option>
        </select>
        <span class="help"><code>IMAGE_FALLBACK_PROVIDER</code> {{ L('dipakai hanya kalau rute Image Generation gagal.', 'is used only when the Image Generation route fails.') }}</span>
      </label>
      <EnvLock :src="locks.IMAGE_FALLBACK_PROVIDER" />
      <div class="grid three mt16">
        <NumField
          v-model="form.gen"
          :label="L('Batas waktu membuat', 'Generation timeout')"
          setting-key="IMAGE_GENERATION_TIMEOUT_SECS"
          :help="L('detik, maks 600', 'seconds, at most 600')"
          :min="1"
          :max="600"
          :locked="locks.IMAGE_GENERATION_TIMEOUT_SECS"
          :invalid="numInvalid(form.gen)"
        />
        <NumField
          v-model="form.conn"
          :label="L('Batas waktu sambung', 'Connect timeout')"
          setting-key="IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS"
          :help="L('detik, maks 600', 'seconds, at most 600')"
          :min="1"
          :max="600"
          :locked="locks.IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS"
          :invalid="numInvalid(form.conn)"
        />
        <NumField
          v-model="form.dl"
          :label="L('Batas waktu unduh', 'Download timeout')"
          setting-key="IMAGE_DOWNLOAD_TIMEOUT_SECS"
          :help="L('detik, maks 600', 'seconds, at most 600')"
          :min="1"
          :max="600"
          :locked="locks.IMAGE_DOWNLOAD_TIMEOUT_SECS"
          :invalid="numInvalid(form.dl)"
        />
      </div>
    </div>

    <SectionTitle :title="L('Koneksi provider', 'Provider connection')" />
    <div class="card card-body">
      <NumField
        v-model="form.aiConn"
        :label="L('Batas waktu sambung ke provider', 'Provider connect timeout')"
        setting-key="AI_PROVIDER_CONNECT_TIMEOUT_SECS"
        :help="L('detik, maks 600. Berlaku untuk chat, uji model, dan transkripsi.', 'seconds, at most 600. Applies to chat, model tests and transcription.')"
        effect="restart"
        :min="1"
        :max="600"
        :locked="locks.AI_PROVIDER_CONNECT_TIMEOUT_SECS"
        :invalid="numInvalid(form.aiConn)"
      />
    </div>
  </template>

  <CapsSheet :sheet="capsSheet" :role="capsRole" @saved="reload(true)" />
  <ProviderSheet :sheet="providerSheet" :provider="editing" @saved="reload(true)" />
</template>
