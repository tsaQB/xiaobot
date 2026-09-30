<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { ChatSessionView } from '../api/types'
import Brandmark from '../components/Brandmark.vue'
import BusyButton from '../components/BusyButton.vue'
import ChatComposer from '../components/chat/ChatComposer.vue'
import ChatMessage from '../components/chat/ChatMessage.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import Rich from '../components/Rich'
import Sheet from '../components/Sheet.vue'
import { fmtWhen } from '../format'
import { L, nf } from '../i18n'
import {
  attach,
  chat,
  clearSession,
  deleteSession,
  detach,
  loadMessages,
  loadSessions,
  newSession,
  renameSession,
  retry,
  select,
  sessionUi,
  type UiMessage,
} from '../stores/chat'
import { confirmAction, copyText, isDesk, toast, useSheet } from '../stores/ui'

const logEl = ref<HTMLElement | null>(null)
const composer = ref<InstanceType<typeof ChatComposer> | null>(null)

onMounted(attach)
onBeforeUnmount(detach)

/* Desktop: the session list column can be folded away; the choice is remembered per browser. */
const SIDE_KEY = 'xiao-chat-sessions'
function savedSideOpen(): boolean {
  try {
    return localStorage.getItem(SIDE_KEY) !== 'collapsed'
  } catch {
    return true
  }
}
const sideOpen = ref(savedSideOpen())
function toggleSide(): void {
  sideOpen.value = !sideOpen.value
  try {
    localStorage.setItem(SIDE_KEY, sideOpen.value ? 'open' : 'collapsed')
  } catch {
    /* storage may be blocked */
  }
}

const active = computed<ChatSessionView | null>(() => chat.sessions.find((s) => s.id === chat.activeId) ?? null)
const u = computed(() => (chat.activeId === null ? null : sessionUi(chat.activeId)))
const messages = computed<UiMessage[] | null>(() => u.value?.messages ?? null)
const busy = computed(() => !!u.value && (u.value.streaming || u.value.remoteBusy))
const count = computed(() => {
  const list = messages.value
  return list ? list.filter((m) => m.key !== 'busy' && !m.live).length : (active.value?.messages ?? 0)
})

const isBusy = (s: ChatSessionView): boolean => s.busy || !!chat.ui[s.id]?.streaming

/* Autoscroll: follow new output only while the reader is near the end. */
function nearBottom(): boolean {
  const log = logEl.value
  if (!log) return false
  if (isDesk()) return log.scrollHeight - log.scrollTop - log.clientHeight < 96
  const doc = document.documentElement
  return doc.scrollHeight - window.scrollY - window.innerHeight < 200
}

function toBottom(): void {
  const log = logEl.value
  if (!log) return
  if (isDesk()) log.scrollTop = log.scrollHeight
  else window.scrollTo(0, document.documentElement.scrollHeight)
}

watch(
  () => chat.version,
  () => {
    const stick = nearBottom()
    void nextTick(() => {
      if (stick) toBottom()
    })
  },
  { flush: 'pre' },
)
watch(
  () => [chat.jump, chat.activeId, messages.value === null] as const,
  () => void nextTick(toBottom),
)
onMounted(() => void nextTick(toBottom))

function pick(e: Event): void {
  const id = Number((e.target as HTMLSelectElement).value)
  if (Number.isInteger(id) && id !== chat.activeId) select(id)
}

async function makeNew(): Promise<void> {
  await newSession()
  await nextTick()
  composer.value?.focus()
}

async function copy(m: UiMessage): Promise<void> {
  const ok = await copyText(m.text)
  toast(ok ? L('Jawaban disalin', 'Answer copied') : L('Tidak bisa menyalin', 'Could not copy'), !ok)
}

/* Session menu, rename, clear, delete */
const menu = useSheet()
const renameSheet = useSheet()
const newName = ref('')

function openRename(): void {
  newName.value = active.value?.name ?? ''
  renameSheet.show()
}

async function saveName(): Promise<void> {
  const s = active.value
  const name = newName.value.trim()
  if (!s) return
  if (!name) {
    toast(L('Nama tidak boleh kosong', 'The name cannot be empty'), true)
    return
  }
  await renameSession(s.id, name.slice(0, 60))
  renameSheet.hide()
}

function askClear(): void {
  const s = active.value
  if (!s) return
  confirmAction({
    title: L('Kosongkan sesi ini?', 'Clear this session?'),
    text: L(
      `${nf(count.value)} pesan di sesi #${s.id} dihapus dari database. Memori jangka panjang tetap ada.`,
      `${nf(count.value)} messages in session #${s.id} are deleted from the database. Long-term memory is kept.`,
    ),
    label: L('Kosongkan', 'Clear'),
    run: () => clearSession(s.id),
  })
}

function askDelete(): void {
  const s = active.value
  if (!s) return
  confirmAction({
    title: L('Hapus sesi?', 'Delete this session?'),
    text: L(`Sesi #${s.id} dan riwayatnya dihapus, juga dari daftar \`xiao chat\`.`, `Session #${s.id} and its history are deleted, also from the \`xiao chat\` list.`),
    label: L('Hapus', 'Delete'),
    run: () => deleteSession(s.id),
  })
}
</script>

<template>
  <h2 class="sr">Chat</h2>
  <LoadState v-if="!chat.loaded" :loading="chat.loading || !chat.error" :error="chat.error" @retry="loadSessions()" />

  <div v-else class="chat" :class="{ 'side-closed': !sideOpen }">
    <aside id="chat-sessions" class="chat-side" :aria-label="L('Sesi chat', 'Chat sessions')">
      <div class="chat-side-head">
        <h3>{{ L('Sesi', 'Sessions') }}</h3>
        <BusyButton class="btn sm" :run="makeNew" :label="L('Membuat…', 'Creating…')"><Icon name="plus" size="sm" />{{ L('Baru', 'New') }}</BusyButton>
      </div>
      <nav class="sess-list" :aria-label="L('Daftar sesi', 'Session list')">
        <a
          v-for="s in chat.sessions"
          :key="s.id"
          class="sess"
          :class="{ on: s.id === chat.activeId }"
          href="#/chat"
          :aria-current="s.id === chat.activeId ? 'true' : undefined"
          @click.prevent="s.id !== chat.activeId && select(s.id)"
        >
          <span class="grow">
            <span class="n">{{ s.name }}</span>
            <span class="s">{{ isBusy(s) ? L('Menjawab…', 'Answering…') : `${nf(s.messages)} ${L('pesan', 'messages')}, ${fmtWhen(s.last_at ?? s.created_at)}` }}</span>
          </span>
          <span v-if="isBusy(s)" class="spin-inline" aria-hidden="true"></span>
          <span v-else class="id">#{{ s.id }}</span>
        </a>
      </nav>
      <p class="chat-side-foot small faint"><Rich :text="L('Daftar yang sama dengan `xiao chat` di terminal.', 'The same list as `xiao chat` in the terminal.')" /></p>
    </aside>

    <div class="chat-bar only-phone">
      <select class="select" :value="chat.activeId ?? ''" :aria-label="L('Pilih sesi', 'Pick a session')" @change="pick">
        <option v-for="s in chat.sessions" :key="s.id" :value="s.id">#{{ s.id }} {{ s.name }}{{ isBusy(s) ? ` (${L('menjawab', 'answering')})` : '' }}</option>
      </select>
      <BusyButton class="iconbtn" :run="makeNew" label="" :title="L('Sesi baru', 'New session')" :aria-label="L('Sesi baru', 'New session')">
        <Icon name="plus" />
      </BusyButton>
      <button type="button" class="iconbtn" :title="L('Menu sesi', 'Session menu')" :aria-label="L('Menu sesi', 'Session menu')" @click="menu.show()">
        <Icon name="dots" />
      </button>
    </div>

    <section class="chat-main" :aria-label="active?.name ?? 'Chat'">
      <div class="chat-head">
        <button
          type="button"
          class="iconbtn side-toggle"
          aria-controls="chat-sessions"
          :aria-expanded="sideOpen"
          :title="sideOpen ? L('Sembunyikan daftar sesi', 'Hide the session list') : L('Tampilkan daftar sesi', 'Show the session list')"
          :aria-label="L('Daftar sesi', 'Session list')"
          @click="toggleSide"
        >
          <Icon name="panel" />
        </button>
        <div class="grow">
          <h3>{{ active?.name ?? '…' }}</h3>
          <div class="sub">
            {{ nf(count) }} {{ L('pesan', 'messages') }}, model <span class="mono">{{ chat.model ?? L('belum dipilih', 'not chosen') }}</span>
          </div>
        </div>
        <BusyButton
          v-if="!sideOpen"
          class="iconbtn"
          :run="makeNew"
          label=""
          :title="L('Sesi baru', 'New session')"
          :aria-label="L('Sesi baru', 'New session')"
        >
          <Icon name="plus" />
        </BusyButton>
        <button type="button" class="iconbtn" :title="L('Menu sesi', 'Session menu')" :aria-label="L('Menu sesi', 'Session menu')" @click="menu.show()">
          <Icon name="dots" />
        </button>
      </div>

      <div ref="logEl" class="chat-log">
        <LoadState
          v-if="!messages"
          :loading="!!u?.loading || !u?.error"
          :error="u?.error ?? null"
          @retry="chat.activeId !== null && loadMessages(chat.activeId)"
        />
        <div v-else-if="!messages.length" class="chat-empty">
          <Brandmark />
          <b>{{ L('Sesi ini masih kosong', 'This session is empty') }}</b>
          <p>
            <Rich
              :text="L('Sesi di sini sama dengan sesi `xiao chat` di terminal. Model, memori, dan tool-nya sama dengan di Telegram.', 'Sessions here are the same as `xiao chat` in the terminal. The model, memory and tools match Telegram.')"
            />
          </p>
          <p>
            {{ L('Kuis tampil sebagai teks. Foto dan tautan media tampil di jawaban, dan berkas buatan Xiao bisa diunduh.', 'Quizzes arrive as text. Photos and media links appear in the answer, and files Xiao creates can be downloaded.') }}
          </p>
        </div>
        <template v-else>
          <ChatMessage
            v-for="(m, i) in messages"
            :key="m.key"
            :m="m"
            :last="i === messages.length - 1"
            :busy="busy"
            @retry="retry(m.key)"
            @copy="copy(m)"
          />
        </template>
      </div>

      <ChatComposer ref="composer" />
      <div class="sr" aria-live="polite">{{ chat.live }}</div>
    </section>
  </div>

  <Sheet :sheet="menu" :title="active?.name ?? ''">
    <template #sub>
      <Rich
        v-if="active"
        :text="L(`Sesi #${active.id}, ${nf(count)} pesan. Juga terlihat di \`xiao chat\`.`, `Session #${active.id}, ${nf(count)} messages. Also listed in \`xiao chat\`.`)"
      />
    </template>
    <div class="sheet-rows rows">
      <button type="button" class="row" @click="openRename">
        <span class="row-ic"><Icon name="edit" /></span>
        <span class="grow"><span class="label">{{ L('Ganti nama', 'Rename') }}</span><span class="hint">{{ L('Hanya nama yang berubah.', 'Only the name changes.') }}</span></span>
      </button>
      <button type="button" class="row" :disabled="!count || busy" @click="askClear">
        <span class="row-ic"><Icon name="refresh" /></span>
        <span class="grow">
          <span class="label">{{ L('Kosongkan riwayat', 'Clear history') }}</span><span class="hint">{{ L('Sesi tetap ada, pesannya dihapus.', 'The session stays; its messages are deleted.') }}</span>
        </span>
      </button>
      <button type="button" class="row danger-row" :disabled="busy" @click="askDelete">
        <span class="row-ic"><Icon name="trash" /></span>
        <span class="grow"><span class="label">{{ L('Hapus sesi', 'Delete session') }}</span><span class="hint">{{ L('Sesi dan riwayatnya dihapus.', 'The session and its history are deleted.') }}</span></span>
      </button>
    </div>
  </Sheet>

  <Sheet :sheet="renameSheet" :title="L('Ganti nama sesi', 'Rename session')">
    <template #sub>
      <Rich
        v-if="active"
        :text="L(`Sesi #${active.id}. Nama baru juga tampil di \`xiao chat\`.`, `Session #${active.id}. The new name also shows in \`xiao chat\`.`)"
      />
    </template>
    <form @submit.prevent>
      <label class="field mt0">
        <span class="lab">{{ L('Nama', 'Name') }}</span>
        <input v-model="newName" class="input" maxlength="60" autocomplete="off" />
      </label>
      <div class="actions end mt16">
        <button type="button" class="btn ghost" @click="renameSheet.hide()">{{ L('Batal', 'Cancel') }}</button>
        <BusyButton type="submit" class="btn primary" :run="saveName" :label="L('Menyimpan…', 'Saving…')">{{ L('Simpan', 'Save') }}</BusyButton>
      </div>
    </form>
  </Sheet>
</template>
