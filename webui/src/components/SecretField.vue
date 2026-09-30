<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { api, seg } from '../api/client'
import type { SecretMeta, SecretWriteResult, WriteResult } from '../api/types'
import { L } from '../i18n'
import { confirmAction, toast } from '../stores/ui'
import BusyButton from './BusyButton.vue'
import EnvLock from './EnvLock.vue'
import Icon from './Icon.vue'

/*
 * A secret is never sent to the browser: the field shows a mask with the last
 * four characters and where it is stored, and offers Set/Replace and Delete.
 */
const props = defineProps<{
  secretKey: string
  meta: SecretMeta
  locked?: string | null
  whereText?: string
  deleteWarn?: string
  savedToast?: (r: SecretWriteResult) => string
  /** Set/Replace emits `edit` instead of the inline editor (the web password uses a sheet). */
  external?: boolean
  deleteFn?: () => Promise<unknown>
}>()

const emit = defineEmits<{ changed: []; edit: [] }>()

const editing = ref(false)
const value = ref('')
const input = ref<HTMLInputElement | null>(null)

const readOnly = computed(() => props.meta.where === 'environment' || !!props.locked)
const where = computed(() => {
  if (props.whereText) return props.whereText
  if (props.meta.where === 'environment') return L('dari environment, hanya baca', 'from the environment, read-only')
  if (props.meta.where === 'vault') return L('disimpan di vault', 'stored in the vault')
  return ''
})

async function startEdit(): Promise<void> {
  if (props.external) {
    emit('edit')
    return
  }
  value.value = ''
  editing.value = true
  await nextTick()
  input.value?.focus()
}

function cancel(): void {
  editing.value = false
  value.value = ''
}

async function save(): Promise<void> {
  const v = value.value.trim()
  if (!v) {
    toast(L('Isi dulu nilainya', 'Enter a value first'), true)
    input.value?.focus()
    return
  }
  const r = await api.put<SecretWriteResult>(`/api/secrets/${seg(props.secretKey)}`, { value: v })
  editing.value = false
  value.value = ''
  toast(props.savedToast ? props.savedToast(r) : L('Tersimpan di vault dan langsung dipakai.', 'Stored in the vault and in use now.'))
  emit('changed')
}

function askDelete(): void {
  const key = props.secretKey
  const warn = props.deleteWarn ?? L('Mesin atau provider ini tidak akan dipakai lagi.', 'This engine or provider will no longer be used.')
  confirmAction({
    title: L('Hapus kunci ini?', 'Delete this key?'),
    text: L(`\`${key}\` dihapus dari vault. ${warn}`, `\`${key}\` is removed from the vault. ${warn}`),
    label: L('Hapus', 'Delete'),
    run: async () => {
      if (props.deleteFn) await props.deleteFn()
      else await api.del<WriteResult>(`/api/secrets/${seg(key)}`)
      toast(L('Kunci dihapus dari vault', 'Key removed from the vault'))
      emit('changed')
    },
  })
}

const busyLabel = computed(() => (props.secretKey === 'BOT_TOKEN' ? L('Memeriksa…', 'Checking…') : L('Menyimpan…', 'Saving…')))
</script>

<template>
  <div v-if="!editing" class="secret" :class="{ unset: !meta.set }">
    <Icon name="key" size="sm" />
    <span class="mask">{{ meta.set ? `••••••••${meta.tail}` : L('belum diisi', 'not set') }}</span>
    <span class="grow">{{ meta.set ? where : '' }}</span>
    <button type="button" class="btn sm" :disabled="readOnly" @click="startEdit">{{ meta.set ? L('Ganti', 'Replace') : L('Isi', 'Set') }}</button>
    <button
      v-if="meta.set"
      type="button"
      class="btn sm ghost"
      :disabled="readOnly"
      :aria-label="`${L('Hapus', 'Delete')} ${secretKey}`"
      :title="L('Hapus', 'Delete')"
      @click="askDelete"
    >
      <Icon name="trash" size="sm" />
    </button>
  </div>
  <div v-else>
    <form class="inline" @submit.prevent>
      <input
        ref="input"
        v-model="value"
        class="input mono"
        type="password"
        autocomplete="off"
        spellcheck="false"
        :placeholder="L('Tempel nilai baru', 'Paste the new value')"
        :aria-label="secretKey"
        @keydown.esc.stop="cancel"
      />
      <BusyButton type="submit" class="btn primary" :run="save" :label="busyLabel">{{ L('Simpan', 'Save') }}</BusyButton>
    </form>
    <div class="flex mt8">
      <button type="button" class="btn sm ghost" @click="cancel">{{ L('Batal', 'Cancel') }}</button>
      <span class="small muted">{{ L('Setelah disimpan, nilai tidak pernah dikirim balik ke browser.', 'Once saved, the value is never sent back to the browser.') }}</span>
    </div>
  </div>
  <EnvLock :src="locked" />
</template>
