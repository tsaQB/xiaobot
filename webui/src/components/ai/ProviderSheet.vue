<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api, seg } from '../../api/client'
import type { ProviderCreateRequest, ProviderTestRequest, ProviderTestResult, ProviderUpdateRequest, ProviderView } from '../../api/types'
import { L, lang, nf } from '../../i18n'
import { hostOf } from '../../lib/ai'
import { toast, type SheetHandle } from '../../stores/ui'
import BusyButton from '../BusyButton.vue'
import EffectBadge from '../EffectBadge.vue'
import Icon from '../Icon.vue'
import Sheet from '../Sheet.vue'

/* Add a provider (Save only after a successful test) or edit one. */
const props = defineProps<{ sheet: SheetHandle; provider: ProviderView | null }>()
const emit = defineEmits<{ saved: [] }>()

const name = ref('')
const endpoint = ref('')
const key = ref('')
const result = ref<ProviderTestResult | null>(null)
const testedFor = ref('')

watch(
  () => props.sheet.open,
  (open) => {
    if (!open) return
    name.value = props.provider?.name ?? ''
    endpoint.value = props.provider?.endpoint ?? ''
    key.value = ''
    result.value = null
    testedFor.value = ''
  },
)

const fingerprint = computed(() => `${endpoint.value.trim()}\n${key.value}`)
const canSave = computed(() => {
  if (props.provider) return true
  return !!result.value?.ok && testedFor.value === fingerprint.value
})

async function test(): Promise<void> {
  const ep = endpoint.value.trim()
  if (!ep) {
    toast(L('Isi endpoint dulu', 'Enter the endpoint first'), true)
    return
  }
  const body: ProviderTestRequest = { endpoint: ep }
  if (key.value.trim()) body.api_key = key.value.trim()
  if (props.provider) body.provider_id = props.provider.id
  result.value = await api.post<ProviderTestResult>('/api/ai/providers/test', body)
  testedFor.value = fingerprint.value
}

async function save(): Promise<void> {
  const ep = endpoint.value.trim()
  if (!ep) {
    toast(L('Isi endpoint dulu', 'Enter the endpoint first'), true)
    return
  }
  if (props.provider) {
    const body: ProviderUpdateRequest = { name: name.value.trim() || props.provider.name, endpoint: ep }
    if (key.value.trim()) body.api_key = key.value.trim()
    await api.put<ProviderView>(`/api/ai/providers/${seg(props.provider.id)}`, body)
  } else {
    const body: ProviderCreateRequest = { name: name.value.trim() || hostOf(result.value?.endpoint ?? ep), endpoint: ep, api_key: key.value.trim() }
    const first = result.value?.models[0]
    if (first) body.model = first
    await api.post<ProviderView>('/api/ai/providers', body)
  }
  props.sheet.hide()
  toast(L('Provider disimpan', 'Provider saved'))
  emit('saved')
}
</script>

<template>
  <Sheet
    :sheet="sheet"
    :title="provider ? L('Ubah provider', 'Edit provider') : L('Tambah provider', 'Add a provider')"
    :sub="L('Endpoint kompatibel OpenAI (chat/completions). Katalog model diambil otomatis.', 'An OpenAI-compatible endpoint (chat/completions). The model catalogue is fetched for you.')"
  >
    <form @submit.prevent>
      <label class="field">
        <span class="lab">{{ L('Nama', 'Name') }}</span>
        <input v-model="name" class="input" :placeholder="L('mis. OpenRouter', 'e.g. OpenRouter')" autocomplete="off" />
      </label>
      <label class="field">
        <span class="lab">Endpoint</span>
        <input v-model="endpoint" class="input mono" inputmode="url" autocomplete="off" spellcheck="false" placeholder="https://openrouter.ai/api/v1" />
      </label>
      <label class="field">
        <span class="lab">API key <EffectBadge type="secret" /></span>
        <input
          v-model="key"
          class="input mono"
          type="password"
          autocomplete="off"
          aria-label="API key"
          :placeholder="provider ? L('Kosongkan untuk tetap memakai kunci lama', 'Leave empty to keep the current key') : L('sk-… atau kosong untuk endpoint lokal', 'sk-… or empty for a local endpoint')"
        />
      </label>
    </form>

    <template v-if="result">
      <div v-if="result.ok" class="note ok mt12">
        <Icon name="check" size="sm" />
        <div v-if="lang === 'en'">Connected in {{ nf(result.ms) }} ms. <b>{{ nf(result.models.length) }} {{ result.models.length === 1 ? 'model' : 'models' }}</b> found.</div>
        <div v-else>Terhubung dalam {{ nf(result.ms) }} ms. <b>{{ nf(result.models.length) }} model</b> ditemukan.</div>
      </div>
      <div v-else class="note err mt12">
        <Icon name="alert" size="sm" /><div>{{ result.error ?? L('Endpoint tidak menjawab.', 'The endpoint did not answer.') }}</div>
      </div>
    </template>

    <div class="actions end mt16">
      <button type="button" class="btn ghost" @click="sheet.hide()">{{ L('Batal', 'Cancel') }}</button>
      <BusyButton class="btn" :run="test" :label="L('Menghubungi…', 'Connecting…')">
        <Icon name="play" size="sm" />{{ L('Tes dan ambil katalog', 'Test and fetch catalogue') }}
      </BusyButton>
      <BusyButton
        class="btn primary"
        :run="save"
        :disabled="!canSave"
        :label="L('Menyimpan…', 'Saving…')"
        :title="canSave ? undefined : L('Tes endpoint dulu', 'Test the endpoint first')"
      >
        {{ L('Simpan', 'Save') }}
      </BusyButton>
    </div>
  </Sheet>
</template>
