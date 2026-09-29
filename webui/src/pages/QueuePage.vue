<script setup lang="ts">
import { computed } from 'vue'
import { api, seg } from '../api/client'
import type { FailedRow, Ok, QueueCounts, QueueKind, QueueState } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import { useLoad } from '../composables/useLoad'
import { fmtWhen } from '../format'
import { L, nf } from '../i18n'
import { shell } from '../stores/session'
import { toast } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<QueueState>('/api/queue'), {
  pollMs: 10_000,
  onData: (d) => {
    shell.failed = d.telegram.failed + d.whatsapp.failed
  },
})

const channels = computed<{ name: string; q: QueueCounts }[]>(() =>
  data.value
    ? [
        { name: 'Telegram', q: data.value.telegram },
        { name: 'WhatsApp', q: data.value.whatsapp },
      ]
    : [],
)

function kind(k: QueueKind): string {
  const t: Record<QueueKind, string> = {
    message: L('pesan', 'message'),
    edited: L('diedit', 'edited'),
    guest: 'guest',
    inline: 'inline',
    inline_result: L('hasil inline', 'inline result'),
    callback: 'callback',
    stop: 'stop',
    other: L('lainnya', 'other'),
  }
  return t[k]
}

function scope(r: FailedRow): string {
  let s: string
  if (r.private) s = L('Chat pribadi', 'Private chat')
  else if (r.thread_id && r.chat_id) s = L(`Grup ${r.chat_id}, topik #${r.thread_id}`, `Group ${r.chat_id}, topic #${r.thread_id}`)
  else if (r.thread_id) s = L(`Topik #${r.thread_id}`, `Topic #${r.thread_id}`)
  else if (r.chat_id && r.chat_id.startsWith('-')) s = L(`Grup ${r.chat_id}`, `Group ${r.chat_id}`)
  else if (r.chat_id) s = `Chat ${r.chat_id}`
  else s = r.kind === 'inline' || r.kind === 'inline_result' ? 'Inline' : '—'
  return r.channel === 'whatsapp' ? `WhatsApp · ${s}` : s
}

const c = computed(() => ({
  upd: 'Update',
  kind: L('Jenis', 'Kind'),
  scope: L('Percakapan', 'Conversation'),
  att: L('Coba', 'Tries'),
  when: L('Waktu', 'Time'),
  why: L('Alasan', 'Reason'),
}))

async function retry(r: FailedRow): Promise<void> {
  await api.post<Ok>(`/api/queue/${seg(r.channel)}/${seg(r.id)}/retry`)
  toast(L(`Update ${r.id} dikembalikan ke antrean`, `Update ${r.id} is back in the queue`))
  await reload(true)
}

async function dismiss(r: FailedRow): Promise<void> {
  await api.del<Ok>(`/api/queue/${seg(r.channel)}/${seg(r.id)}`)
  toast(L(`Update ${r.id} ditandai selesai`, `Update ${r.id} marked as done`))
  await reload(true)
}

function noRetryTitle(r: FailedRow): string {
  return r.kind === 'inline'
    ? L('Inline query yang sudah lewat tidak bisa dijawab lagi', 'An expired inline query cannot be answered again')
    : L('Update ini tidak bisa dicoba lagi', 'This update cannot be retried')
}
</script>

<template>
  <PageHead
    :title="L('Antrean', 'Queue')"
    :text="L('Setiap pesan masuk disimpan dulu ke database, baru diproses. Yang gagal berulang dikarantina di sini.', 'Every incoming message is saved to the database before it is handled. Messages that keep failing are quarantined here.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <template v-for="ch in channels" :key="ch.name">
      <SectionTitle :title="ch.name" />
      <dl class="card qstats">
        <div><dt>{{ L('Menunggu', 'Pending') }}</dt><dd>{{ nf(ch.q.pending) }}</dd></div>
        <div><dt>{{ L('Diproses', 'Processing') }}</dt><dd>{{ nf(ch.q.processing) }}</dd></div>
        <div><dt>{{ L('Selesai', 'Completed') }}</dt><dd>{{ nf(ch.q.completed) }}</dd></div>
        <div :class="{ err: ch.q.failed }"><dt>{{ L('Gagal', 'Failed') }}</dt><dd>{{ nf(ch.q.failed) }}</dd></div>
      </dl>
    </template>

    <SectionTitle :title="L('Dikarantina', 'Quarantined')">
      <Badge v-if="data.failed.length" kind="err">{{ nf(data.failed.length) }}{{ L(' update', data.failed.length === 1 ? ' update' : ' updates') }}</Badge>
    </SectionTitle>
    <template v-if="data.failed.length">
      <div class="card">
        <table class="tbl">
          <thead>
            <tr>
              <th>{{ c.upd }}</th>
              <th>{{ c.kind }}</th>
              <th>{{ c.scope }}</th>
              <th>{{ c.att }}</th>
              <th>{{ c.when }}</th>
              <th>{{ c.why }}</th>
              <th><span class="sr">{{ L('Tindakan', 'Actions') }}</span></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="r in data.failed" :key="`${r.channel}:${r.id}`">
              <td :data-l="c.upd" class="mono break">{{ r.id }}</td>
              <td :data-l="c.kind">{{ kind(r.kind) }}</td>
              <td :data-l="c.scope">{{ scope(r) }}</td>
              <td :data-l="c.att">{{ r.attempts }}×</td>
              <td :data-l="c.when">{{ fmtWhen(r.received_at) }}</td>
              <td :data-l="c.why" class="full"><span class="small err-text">{{ r.error ?? '—' }}</span></td>
              <td class="full">
                <div class="actions">
                  <BusyButton
                    class="btn sm"
                    :run="() => retry(r)"
                    :disabled="!r.retryable"
                    :title="r.retryable ? undefined : noRetryTitle(r)"
                    :label="L('Memproses…', 'Working…')"
                  >
                    <Icon name="refresh" size="sm" />{{ L('Coba lagi', 'Retry') }}
                  </BusyButton>
                  <BusyButton class="btn sm ghost" :run="() => dismiss(r)" :label="L('Memproses…', 'Working…')">{{ L('Abaikan', 'Dismiss') }}</BusyButton>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="note warn mt12">
        <Icon name="alert" size="sm" />
        <div>
          {{ L('“Coba lagi” memproses ulang pesan dari awal. Bila sebagian jawaban sempat terkirim sebelum gagal, bagian itu bisa terkirim dua kali.', '“Retry” handles the message again from the start. If part of the answer went out before the failure, that part can arrive twice.') }}
        </div>
      </div>
    </template>
    <div v-else class="card">
      <div class="empty">
        <Icon name="check" size="lg" />
        <div class="empty-title mt8">{{ L('Tidak ada update yang dikarantina.', 'No updates are quarantined.') }}</div>
      </div>
    </div>
  </template>
</template>
