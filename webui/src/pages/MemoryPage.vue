<script setup lang="ts">
import { computed, ref } from 'vue'
import { api, seg } from '../api/client'
import type { MemoryState, MemoryView, Ok } from '../api/types'
import BusyButton from '../components/BusyButton.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import Sheet from '../components/Sheet.vue'
import { useLoad } from '../composables/useLoad'
import { fmtDay } from '../format'
import { L, lang, nf } from '../i18n'
import { toast, useSheet } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<MemoryState>('/api/memory'))

const q = ref('')
const list = computed(() => {
  const all = data.value?.memories ?? []
  const needle = q.value.trim().toLowerCase()
  return needle ? all.filter((m) => `${m.key} ${m.fact}`.toLowerCase().includes(needle)) : all
})
const maxChars = computed(() => data.value?.max_chars ?? 300)

/* Add / edit */
const edit = useSheet()
const editing = ref<MemoryView | null>(null)
const key = ref('')
const fact = ref('')

function openEdit(m: MemoryView | null): void {
  editing.value = m
  key.value = m?.key ?? ''
  fact.value = m?.fact ?? ''
  edit.show()
}

const normKey = (k: string): string => k.trim().toLowerCase().replace(/\s+/g, '_')

async function saveMemory(): Promise<void> {
  const k = editing.value ? editing.value.key : normKey(key.value)
  const f = fact.value.trim()
  if (!k || !f) {
    toast(L('Kunci dan fakta wajib diisi', 'Both the key and the fact are required'), true)
    return
  }
  if (k.length > 64) {
    toast(L('Kunci paling banyak 64 karakter', 'The key can have at most 64 characters'), true)
    return
  }
  await api.put<Ok>(`/api/memory/${seg(k)}`, { fact: f })
  edit.hide()
  toast(L('Memori disimpan', 'Memory saved'))
  await reload(true)
}

async function remove(m: MemoryView): Promise<void> {
  await api.del<Ok>(`/api/memory/${seg(m.key)}`)
  toast(L(`Memori “${m.key}” dihapus`, `Memory “${m.key}” deleted`))
  await reload(true)
}

/* Delete every fact, with a typed confirmation */
const clear = useSheet()
const typed = ref('')
const word = computed(() => L('HAPUS', 'DELETE'))

function openClear(): void {
  typed.value = ''
  clear.show()
}

async function clearAll(): Promise<void> {
  await api.del<Ok>('/api/memory')
  clear.hide()
  toast(L('Semua memori dihapus', 'Every memory deleted'))
  await reload(true)
}
</script>

<template>
  <PageHead
    :title="L('Memori', 'Memory')"
    :text="L('Fakta jangka panjang tentang owner. Curator mengisinya otomatis dari percakapan; di sini bisa dikoreksi.', 'Long-term facts about the owner. The curator fills them in from conversations; you can correct them here.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <div class="toolbar">
      <input v-model="q" class="input" type="search" :placeholder="L('Cari memori…', 'Search memories…')" :aria-label="L('Cari memori', 'Search memories')" />
      <button type="button" class="btn primary" @click="openEdit(null)"><Icon name="plus" size="sm" />{{ L('Tambah', 'Add') }}</button>
    </div>
    <div class="card rows">
      <div v-for="m in list" :key="m.key" class="row top">
        <div class="grow">
          <div class="mono small faint">{{ m.key }}</div>
          <div class="break">{{ m.fact }}</div>
          <div class="hint">{{ L('Diperbarui', 'Updated') }} {{ fmtDay(m.updated_at) }}</div>
        </div>
        <button type="button" class="btn sm ghost" :aria-label="`${L('Ubah', 'Edit')} ${m.key}`" :title="L('Ubah', 'Edit')" @click="openEdit(m)">
          <Icon name="edit" size="sm" />
        </button>
        <BusyButton class="btn sm ghost" :run="() => remove(m)" label="" :aria-label="`${L('Hapus', 'Delete')} ${m.key}`" :title="L('Hapus', 'Delete')">
          <Icon name="trash" size="sm" />
        </BusyButton>
      </div>
      <div v-if="!list.length" class="empty">
        {{ data.memories.length ? L('Tidak ada memori yang cocok.', 'No memories match.') : L('Belum ada memori.', 'No memories yet.') }}
      </div>
    </div>
    <div class="small muted mt8">
      {{
        L(
          `Paling banyak ${nf(data.max_prompt)} fakta masuk ke prompt, masing-masing dipotong ${nf(data.max_chars)} karakter. Isi memori diperlakukan sebagai data, bukan perintah.`,
          `At most ${nf(data.max_prompt)} facts go into the prompt, each cut to ${nf(data.max_chars)} characters. Memory content is treated as data, never as instructions.`,
        )
      }}
    </div>

    <SectionTitle :title="L('Zona berbahaya', 'Danger zone')" />
    <div class="card card-body danger-zone">
      <div class="grow">
        <div class="label">{{ L('Hapus semua memori', 'Delete every memory') }}</div>
        <div class="small muted">{{ L('Riwayat chat dan ringkasan tidak ikut terhapus.', 'Chat history and summaries are kept.') }}</div>
      </div>
      <button type="button" class="btn danger" :disabled="!data.memories.length" @click="openClear">
        <Icon name="trash" size="sm" />{{ L('Hapus semua', 'Delete all') }}
      </button>
    </div>
  </template>

  <Sheet
    :sheet="edit"
    :title="editing ? L('Ubah memori', 'Edit memory') : L('Tambah memori', 'Add a memory')"
    :sub="L('Tulis sebagai fakta singkat tentang owner.', 'Write it as a short fact about the owner.')"
  >
    <form @submit.prevent>
      <label class="field">
        <span class="lab">{{ L('Kunci', 'Key') }}</span>
        <input v-model="key" class="input mono" maxlength="64" autocomplete="off" :placeholder="L('mis. hobi', 'e.g. hobby')" :disabled="!!editing" />
        <span v-if="!editing && key && normKey(key) !== key" class="help">{{ L('Disimpan sebagai', 'Saved as') }} <code>{{ normKey(key) }}</code></span>
      </label>
      <label class="field">
        <span class="lab">{{ L('Fakta', 'Fact') }}</span>
        <textarea v-model="fact" class="input" :maxlength="maxChars" :placeholder="L('mis. Suka fotografi jalanan.', 'e.g. Enjoys street photography.')"></textarea>
        <span class="help counter">{{ nf(fact.length) }}/{{ nf(maxChars) }}</span>
      </label>
      <div class="actions end mt16">
        <button type="button" class="btn ghost" @click="edit.hide()">{{ L('Batal', 'Cancel') }}</button>
        <BusyButton type="submit" class="btn primary" :run="saveMemory" :label="L('Menyimpan…', 'Saving…')">{{ L('Simpan', 'Save') }}</BusyButton>
      </div>
    </form>
  </Sheet>

  <Sheet :sheet="clear" :title="L('Hapus semua memori?', 'Delete every memory?')">
    <template #sub>
      <template v-if="lang === 'en'">{{ nf(data?.memories.length ?? 0) }} facts will be deleted for good. Type <b>{{ word }}</b> to continue.</template>
      <template v-else>{{ nf(data?.memories.length ?? 0) }} fakta akan dihapus permanen. Ketik <b>{{ word }}</b> untuk melanjutkan.</template>
    </template>
    <input v-model="typed" class="input mono" autocomplete="off" :placeholder="word" :aria-label="L('Konfirmasi', 'Confirmation')" />
    <div class="actions end mt16">
      <button type="button" class="btn ghost" @click="clear.hide()">{{ L('Batal', 'Cancel') }}</button>
      <BusyButton class="btn danger solid" :run="clearAll" :disabled="typed.trim() !== word" :label="L('Menghapus…', 'Deleting…')">
        {{ L('Hapus semua', 'Delete all') }}
      </BusyButton>
    </div>
  </Sheet>
</template>
