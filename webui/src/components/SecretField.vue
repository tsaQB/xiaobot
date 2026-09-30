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
 * A secret is never sent to the browser. Not set: the input and Save show
 * right away. Set: a mask with the last four characters and where it is
 * stored, plus Replace (opens the input) and Delete.
 */
const props = defineProps<{
  secretKey: string
  meta: SecretMeta
  locked?: string | null
  whereText?: string
  deleteWarn?: string
  savedToast?: (r: SecretWriteResult) => string
  deleteFn?: () => Promise<unknown>
  /** Saves through another endpoint (the web password); `savedText` is its toast. */
  saveFn?: (value: string) => Promise<unknown>
  savedText?: string
  /** Ask for the value twice (passwords). */
  repeat?: boolean
  minLength?: number
  placeholder?: string
  autocomplete?: string
}>()

const emit = defineEmits<{ changed: [] }>()

const replacing = ref(false)
const value = ref('')
const again = ref('')
const problem = ref<string | null>(null)
const input = ref<HTMLInputElement | null>(null)

const readOnly = computed(() => props.meta.where === 'environment' || !!props.locked)
/* The editor shows at once while nothing is stored, and after Replace otherwise. */
const editing = computed(() => !readOnly.value && (!props.meta.set || replacing.value))
const where = computed(() => {
  if (props.whereText) return props.whereText
  if (props.meta.where === 'environment') return L('dari environment, hanya baca', 'from the environment, read-only')
  if (props.meta.where === 'vault') return L('disimpan di vault', 'stored in the vault')
  return ''
})
const hint = computed(() =>
  props.placeholder ?? (props.meta.set ? L('Tempel nilai baru', 'Paste the new value') : L('Tempel nilainya di sini', 'Paste the value here')),
)

async function startReplace(): Promise<void> {
  value.value = ''
  again.value = ''
  problem.value = null
  replacing.value = true
  await nextTick()
  input.value?.focus()
}

function cancel(): void {
  replacing.value = false
  value.value = ''
  again.value = ''
  problem.value = null
}

async function save(): Promise<void> {
  const v = props.repeat ? value.value : value.value.trim()
  if (!v.trim()) {
    problem.value = L('Isi dulu nilainya.', 'Enter a value first.')
    input.value?.focus()
    return
  }
  if (props.minLength && v.length < props.minLength) {
    problem.value = L(`Paling sedikit ${props.minLength} karakter.`, `At least ${props.minLength} characters.`)
    input.value?.focus()
    return
  }
  if (props.repeat && v !== again.value) {
    problem.value = L('Kedua isian tidak sama.', 'The two entries do not match.')
    return
  }
  problem.value = null
  let msg: string
  if (props.saveFn) {
    await props.saveFn(v)
    msg = props.savedText ?? L('Tersimpan.', 'Saved.')
  } else {
    const r = await api.put<SecretWriteResult>(`/api/secrets/${seg(props.secretKey)}`, { value: v })
    msg = props.savedToast ? props.savedToast(r) : L('Tersimpan di vault dan langsung dipakai.', 'Stored in the vault and in use now.')
  }
  cancel()
  toast(msg)
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
  <div v-if="editing">
    <form @submit.prevent>
      <input
        v-if="repeat"
        ref="input"
        v-model="value"
        class="input secret-input"
        type="password"
        :autocomplete="autocomplete ?? 'new-password'"
        spellcheck="false"
        :minlength="minLength"
        :placeholder="hint"
        :aria-label="secretKey"
        @keydown.esc.stop="cancel"
      />
      <div class="inline" :class="{ mt8: repeat }">
        <input
          v-if="repeat"
          v-model="again"
          class="input secret-input"
          type="password"
          :autocomplete="autocomplete ?? 'new-password'"
          spellcheck="false"
          :minlength="minLength"
          :placeholder="L('Ulangi', 'Repeat')"
          :aria-label="L(`Ulangi ${secretKey}`, `Repeat ${secretKey}`)"
          @keydown.esc.stop="cancel"
        />
        <input
          v-else
          ref="input"
          v-model="value"
          class="input mono secret-input"
          type="password"
          :autocomplete="autocomplete ?? 'off'"
          spellcheck="false"
          :placeholder="hint"
          :aria-label="secretKey"
          @keydown.esc.stop="cancel"
        />
        <BusyButton type="submit" class="btn primary" :run="save" :label="busyLabel">{{ L('Simpan', 'Save') }}</BusyButton>
      </div>
    </form>
    <div v-if="problem" class="field-err" role="alert">{{ problem }}</div>
    <div v-if="meta.set || value" class="flex mt8">
      <button v-if="meta.set" type="button" class="btn sm ghost" @click="cancel">{{ L('Batal', 'Cancel') }}</button>
      <span class="small muted">{{ L('Setelah disimpan, nilai tidak pernah dikirim balik ke browser.', 'Once saved, the value is never sent back to the browser.') }}</span>
    </div>
  </div>
  <div v-else class="secret" :class="{ unset: !meta.set }">
    <Icon name="key" size="sm" />
    <span class="mask">{{ meta.set ? `••••••••${meta.tail}` : L('belum diisi', 'not set') }}</span>
    <span class="grow">{{ meta.set ? where : '' }}</span>
    <template v-if="meta.set">
      <button type="button" class="btn sm" :disabled="readOnly" @click="startReplace">{{ L('Ganti', 'Replace') }}</button>
      <button
        type="button"
        class="btn sm ghost"
        :disabled="readOnly"
        :aria-label="`${L('Hapus', 'Delete')} ${secretKey}`"
        :title="L('Hapus', 'Delete')"
        @click="askDelete"
      >
        <Icon name="trash" size="sm" />
      </button>
    </template>
  </div>
  <EnvLock :src="locked" />
</template>
