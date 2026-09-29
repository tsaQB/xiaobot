<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '../api/client'
import type { ContextClearRequest, ContextState, Ok, ScopeView, WriteResult } from '../api/types'
import Badge from '../components/Badge.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import NumField from '../components/NumField.vue'
import PageHead from '../components/PageHead.vue'
import Rich from '../components/Rich'
import SectionTitle from '../components/SectionTitle.vue'
import { intIn, useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { fmtWhen } from '../format'
import { L, nf } from '../i18n'
import { confirmAction, toast, useSaveBar } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<ContextState>('/api/context'))

/* The selection survives reloads by its chat/thread key. */
const selected = ref('')
const keyOf = (s: ScopeView): string => `${s.chat}/${s.thread}`
const scopes = computed(() => data.value?.scopes ?? [])
const scope = computed<ScopeView | null>(() => scopes.value.find((s) => keyOf(s) === selected.value) ?? scopes.value[0] ?? null)

watch(scopes, (list) => {
  const first = list[0]
  if (first && !list.some((s) => keyOf(s) === selected.value)) selected.value = keyOf(first)
})

function label(s: ScopeView): string {
  switch (s.kind) {
    case 'private':
      return L('Chat pribadi (Telegram)', 'Private chat (Telegram)')
    case 'group':
      return L(`Grup ${s.chat}`, `Group ${s.chat}`)
    case 'topic':
      return L(`Grup ${s.chat}, topik #${s.thread}`, `Group ${s.chat}, topic #${s.thread}`)
    case 'cli':
      return L(`Sesi #${s.session_id ?? -s.thread}: ${s.name ?? ''} (CLI dan web)`, `Session #${s.session_id ?? -s.thread}: ${s.name ?? ''} (CLI and web)`)
    default:
      return `Chat ${s.chat}`
  }
}

const tokens = computed(() => {
  const s = scope.value
  if (!s) return null
  const t = s.tokens
  const used = t.system + t.memory + t.summary + t.history
  const budget = Math.max(1, t.budget)
  const pct = (n: number): string => `${Math.min(100, (n / budget) * 100).toFixed(2)}%`
  return {
    used,
    budget: t.budget,
    ratio: used / budget,
    parts: [
      { label: 'System prompt', n: t.system, color: 'var(--violet)', w: pct(t.system) },
      { label: L('Memori', 'Memory'), n: t.memory, color: 'var(--info)', w: pct(t.memory) },
      { label: L('Ringkasan lama', 'Older summary'), n: t.summary, color: 'var(--warn)', w: pct(t.summary) },
      { label: L('Riwayat terbaru', 'Recent history'), n: t.history, color: 'var(--accent)', w: pct(t.history) },
    ],
  }
})

function clear(op: ContextClearRequest['op']): void {
  const s = scope.value
  if (!s) return
  const body: ContextClearRequest = { chat: s.chat, thread: s.thread, op }
  const run = async (): Promise<void> => {
    await api.post<Ok>('/api/context/clear', body)
    toast(op === 'summary' ? L('Ringkasan dihapus', 'Summary deleted') : L('Riwayat dibersihkan', 'History cleared'))
    await reload(true)
  }
  if (op === 'summary') {
    confirmAction({
      title: L('Hapus ringkasan?', 'Delete the summary?'),
      text: L('Ringkasan lama untuk percakapan ini dihapus. Curator akan membuatnya lagi saat diperlukan.', 'The older summary for this conversation is deleted. The curator writes a new one when it is needed.'),
      label: L('Hapus', 'Delete'),
      run,
    })
  } else {
    const name = label(s)
    confirmAction({
      title: L('Bersihkan riwayat?', 'Clear the history?'),
      text: L(`Semua pesan tersimpan di “${name}” dihapus. Memori jangka panjang tidak ikut terhapus.`, `Every stored message in “${name}” is deleted. Long-term memory is kept.`),
      label: L('Bersihkan', 'Clear'),
      run,
    })
  }
}

/* History retention (XIAO_HISTORY_RETENTION, applies at once) */
const { form, dirty, reset } = useForm({ retention: '' })
watch(data, (d) => {
  if (d && !dirty.value) reset({ retention: String(d.retention) })
})
const retentionInvalid = computed(() =>
  form.retention.trim() === '' || intIn(form.retention, 0, 100_000_000) !== null ? null : L('Bilangan bulat 0 atau lebih.', 'A whole number, 0 or more.'),
)

async function save(): Promise<void> {
  const n = intIn(form.retention, 0, 100_000_000)
  if (n === null) throw new Error(L('Batas riwayat harus bilangan bulat 0 atau lebih.', 'The history limit must be a whole number, 0 or more.'))
  await api.put<WriteResult>('/api/settings', { XIAO_HISTORY_RETENTION: String(n) })
  toast(L('Pengaturan disimpan', 'Settings saved'))
  const d = await reload(true)
  if (d) reset({ retention: String(d.retention) })
}

useSaveBar({ dirty, save, discard: () => data.value && reset({ retention: String(data.value.retention) }) })
</script>

<template>
  <PageHead :title="L('Konteks dan sesi', 'Context and sessions')">
    <Rich
      :text="
        L(
          'Isi jendela konteks per chat atau topik, ringkasan lama, dan batas riwayat. Sesi chat WebUI dan `xiao chat` ikut tercantum.',
          'What fills the context window for each chat or topic, the older summary, and the history limit. WebUI and `xiao chat` sessions are listed too.',
        )
      "
    />
  </PageHead>

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <div v-if="!scopes.length" class="card"><div class="empty">{{ L('Belum ada percakapan tersimpan.', 'No stored conversations yet.') }}</div></div>

    <template v-else-if="scope && tokens">
      <label class="field mt0">
        <span class="lab">{{ L('Percakapan', 'Conversation') }}</span>
        <select v-model="selected" class="select">
          <option v-for="s in scopes" :key="keyOf(s)" :value="keyOf(s)">{{ label(s) }} ({{ nf(s.messages) }} {{ L('pesan', 'messages') }})</option>
        </select>
      </label>

      <div class="card card-body mt12">
        <div class="flex top">
          <div class="grow">
            <div class="ctx-total">{{ nf(tokens.used) }} <span>/ {{ nf(tokens.budget) }} token</span></div>
            <div class="small muted">
              {{ L(`${nf(scope.messages)} pesan tersimpan`, `${nf(scope.messages)} stored messages`) }}, chat <span class="mono">{{ scope.chat }}</span>, thread
              <span class="mono">{{ scope.thread }}</span><template v-if="data.model">, model <span class="mono">{{ data.model }}</span></template>
            </div>
          </div>
          <Badge :kind="tokens.ratio > 0.8 ? 'warn' : 'ok'">
            {{ L(`${Math.round(tokens.ratio * 100)}% terpakai`, `${Math.round(tokens.ratio * 100)}% used`) }}
          </Badge>
        </div>
        <div class="bar mt12" role="img" :aria-label="L('Pemakaian token', 'Token use')">
          <i v-for="p in tokens.parts" :key="p.label" :style="{ width: p.w, background: p.color }"></i>
        </div>
        <div class="legend">
          <div v-for="p in tokens.parts" :key="p.label">
            <span class="sw" :style="{ background: p.color }"></span>{{ p.label }}<span class="n">{{ nf(p.n) }}</span>
          </div>
        </div>
      </div>

      <div class="card card-body mt12">
        <div class="card-title">{{ L('Ringkasan lama', 'Older summary') }}</div>
        <template v-if="scope.summary">
          <p class="small muted para">{{ scope.summary }}</p>
          <div v-if="scope.summary_updated_at" class="small faint mt8">{{ L('Diperbarui', 'Updated') }} {{ fmtWhen(scope.summary_updated_at) }}</div>
        </template>
        <p v-else class="small muted para">
          {{ L('Belum ada. Curator merangkum riwayat lama saat percakapan mulai panjang.', 'None yet. The curator summarizes older history once a conversation grows long.') }}
        </p>
        <div class="actions mt12">
          <button type="button" class="btn sm" :disabled="!scope.summary" @click="clear('summary')">
            <Icon name="trash" size="sm" />{{ L('Hapus ringkasan', 'Delete summary') }}
          </button>
          <button type="button" class="btn sm danger" :disabled="!scope.messages" @click="clear('history')">
            <Icon name="trash" size="sm" />{{ L('Bersihkan riwayat percakapan ini', "Clear this conversation's history") }}
          </button>
        </div>
      </div>
    </template>

    <SectionTitle :title="L('Riwayat', 'History')" />
    <div class="card card-body">
      <NumField
        v-model="form.retention"
        :label="L('Pesan disimpan per chat atau topik', 'Messages kept per chat or topic')"
        setting-key="XIAO_HISTORY_RETENTION"
        :help="L('0 = tanpa batas. Pesan tertua dipangkas saat pesan baru disimpan.', '0 = no limit. The oldest messages are pruned when new ones are saved.')"
        :min="0"
        :locked="data.env_locks.XIAO_HISTORY_RETENTION"
        :invalid="retentionInvalid"
      />
    </div>
  </template>
</template>
