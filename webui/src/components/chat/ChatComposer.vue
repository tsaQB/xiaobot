<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { fmtBytes } from '../../format'
import { L } from '../../i18n'
import { addFiles, chat, removePending, send, sessionUi, stop } from '../../stores/chat'
import { isCoarse } from '../../stores/ui'
import Icon from '../Icon.vue'
import { routeIcon, routeLabel } from './chatFiles'

const root = ref<HTMLElement | null>(null)
const input = ref<HTMLTextAreaElement | null>(null)
const picker = ref<HTMLInputElement | null>(null)

const u = computed(() => (chat.activeId === null ? null : sessionUi(chat.activeId)))
const busy = computed(() => !!u.value && (u.value.streaming || u.value.remoteBusy))
const draft = computed({
  get: () => u.value?.draft ?? '',
  set: (v: string) => {
    if (u.value) u.value.draft = v
  },
})

/* The phone composer is fixed; the page keeps room for its current height. */
function syncHeight(): void {
  const el = root.value
  if (el && el.offsetHeight) document.documentElement.style.setProperty('--composer-h', `${el.offsetHeight}px`)
}

function autoGrow(): void {
  const el = input.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${Math.min(el.scrollHeight || 44, 168)}px`
  syncHeight()
}

let observer: ResizeObserver | null = null
onMounted(() => {
  autoGrow()
  if (root.value && 'ResizeObserver' in window) {
    observer = new ResizeObserver(syncHeight)
    observer.observe(root.value)
  }
  window.addEventListener('resize', syncHeight)
})
onBeforeUnmount(() => {
  observer?.disconnect()
  window.removeEventListener('resize', syncHeight)
})

watch(draft, () => void nextTick(autoGrow))
watch(() => chat.activeId, () => void nextTick(autoGrow))

async function doSend(): Promise<void> {
  if (busy.value) return
  const sent = await send()
  if (!sent) input.value?.focus()
}

/* Enter sends on a keyboard; on a touch screen it adds a line, like Telegram. */
function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Enter' || e.shiftKey || e.isComposing || e.keyCode === 229) return
  if (isCoarse()) return
  e.preventDefault()
  void doSend()
}

function onPick(e: Event): void {
  const el = e.target as HTMLInputElement
  addFiles(Array.from(el.files ?? []))
  el.value = ''
}

function focus(): void {
  input.value?.focus()
}

defineExpose({ focus })
</script>

<template>
  <div ref="root" class="composer">
    <div v-if="chat.pending.length" class="chat-files">
      <span v-for="p in chat.pending" :key="p.key" class="fchip">
        <Icon :name="routeIcon(p.route)" size="sm" />
        <span class="fname" :title="p.name">{{ p.name }}</span>
        <span class="fmeta">{{ fmtBytes(p.size) }}, {{ routeLabel(p.route) }}</span>
        <span v-if="p.uploading" class="spin-inline" role="status" :aria-label="L('Mengunggah', 'Uploading')"></span>
        <button type="button" :aria-label="`${L('Lepas', 'Remove')} ${p.name}`" :title="L('Lepas', 'Remove')" @click="removePending(p.key)">
          <Icon name="x" size="sm" />
        </button>
      </span>
    </div>
    <div class="composer-row">
      <input ref="picker" type="file" multiple hidden @change="onPick" />
      <button type="button" class="iconbtn" :title="L('Lampirkan berkas', 'Attach files')" :aria-label="L('Lampirkan berkas', 'Attach files')" @click="picker?.click()">
        <Icon name="clip" />
      </button>
      <textarea
        ref="input"
        v-model="draft"
        class="input"
        rows="1"
        :placeholder="L('Tulis pesan untuk Xiao', 'Write a message to Xiao')"
        :aria-label="L('Pesan', 'Message')"
        enterkeyhint="send"
        @keydown="onKey"
      ></textarea>
      <button v-if="busy" type="button" class="btn" :disabled="u?.stopping" @click="stop">
        <Icon name="stop" size="sm" /><span class="send-t">{{ u?.stopping ? L('Menghentikan…', 'Stopping…') : L('Hentikan', 'Stop') }}</span>
      </button>
      <button v-else type="button" class="btn primary" @click="doSend">
        <Icon name="send" size="sm" /><span class="send-t">{{ L('Kirim', 'Send') }}</span>
      </button>
    </div>
    <div class="composer-hint only-desk">
      {{ L('Enter mengirim, Shift+Enter untuk baris baru. Lampiran sampai 20 MB per berkas.', 'Enter sends, Shift+Enter adds a new line. Attachments up to 20 MB each.') }}
    </div>
  </div>
</template>
