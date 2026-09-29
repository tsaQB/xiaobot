<script setup lang="ts">
import { computed } from 'vue'
import { api } from '../api/client'
import type { AttentionItem, Overview, RoleTestResult, TelegramCheck, WaPhase } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import { useLoad } from '../composables/useLoad'
import { fmtAgoSecs, fmtBytes, fmtDuration, fmtMs, listJoin } from '../format'
import { L, lang, nf } from '../i18n'
import { confirmRestart } from '../lib/actions'
import { applyOverview, uptimeNow } from '../stores/session'
import { toast } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<Overview>('/api/overview'), {
  pollMs: 15_000,
  onData: applyOverview,
})

interface AttentionRow {
  kind: AttentionItem['kind']
  icon: string
  title: string
  hint: string
  to: string
}

function attentionRow(a: AttentionItem, o: Overview): AttentionRow {
  const bot = o.telegram.username ? `@${o.telegram.username}` : 'bot'
  const names = listJoin(a.names ?? [])
  const n = a.count ?? 0
  const base = { kind: a.kind, to: `/${a.to}` }
  switch (a.code) {
    case 'queue_failed':
      return { ...base, icon: 'inbox', title: L(`${n} update gagal diproses`, n === 1 ? '1 update failed' : `${n} updates failed`), hint: L('Lihat alasannya, lalu coba lagi atau abaikan.', 'See why, then retry or dismiss them.') }
    case 'privacy_mode':
      return { ...base, icon: 'send', title: L('Privacy mode grup masih aktif', 'Group privacy mode is still on'), hint: L(`Sebutan ${bot} di grup tidak sampai ke bot.`, `Mentions of ${bot} in groups do not reach the bot.`) }
    case 'inline_feedback':
      return { ...base, icon: 'send', title: L('Inline feedback belum aktif', 'Inline feedback is off'), hint: L('Jawaban inline tertahan di “Xiao sedang berpikir…”.', 'Inline answers stay stuck at “Xiao is thinking…”.') }
    case 'no_search_key': {
      const paused = o.search.paused
      return {
        ...base,
        icon: 'search',
        title: L('Belum ada kunci mesin pencari', 'No search engine key yet'),
        hint: paused.length
          ? L(`${listJoin(paused)} sedang dijeda; hasil jatuh ke ${o.search.first ?? 'Wikipedia'}.`, `${listJoin(paused)} ${paused.length === 1 ? 'is' : 'are'} paused, so results fall back to ${o.search.first ?? 'Wikipedia'}.`)
          : L('Pencarian bergantung pada Exa MCP dan DuckDuckGo tanpa kunci, yang sering dijeda.', 'Search relies on the keyless Exa MCP and DuckDuckGo, which are often paused.'),
      }
    }
    case 'search_paused':
      return { ...base, icon: 'search', title: L(`${names || 'Mesin pencari'} sedang dijeda`, `${names || 'Search engines'} paused`), hint: L('Pencarian memakai mesin berikutnya sampai jedanya habis.', 'Searches use the next engine until the pause ends.') }
    case 'whatsapp_unlinked':
      return { ...base, icon: 'phone', title: L('WhatsApp belum ditautkan', 'WhatsApp is not linked'), hint: L('Tautkan dengan kode pairing langsung dari HP.', 'Link it with a pairing code, right from your phone.') }
    case 'whatsapp_failed':
      return { ...base, icon: 'phone', title: L('Gateway WhatsApp gagal', 'The WhatsApp gateway failed'), hint: L('Gateway dicoba ulang otomatis. Lihat alasannya di halaman WhatsApp.', 'The gateway retries on its own. See why on the WhatsApp page.') }
    case 'restart_needed':
      return { ...base, icon: 'refresh', title: L('Ada perubahan yang menunggu restart', 'Some changes wait for a restart'), hint: L('Restart daemon agar pengaturan baru berlaku.', 'Restart the daemon so the new settings apply.') }
    case 'no_provider':
      return { ...base, icon: 'cpu', title: L('Belum ada provider AI', 'No AI provider yet'), hint: L('Tambahkan provider dan pilih model utama agar Xiao bisa menjawab.', 'Add a provider and pick a main model so Xiao can answer.') }
    case 'telegram_offline':
      return { ...base, icon: 'send', title: L('Telegram tidak tersambung', 'Telegram is not connected'), hint: L('Periksa token bot dan koneksi server, lalu lihat log.', "Check the bot token and the server's connection, then look at the logs.") }
    default:
      return { ...base, icon: 'info', title: a.code, hint: '' }
  }
}

const attention = computed(() => (data.value ? data.value.attention.map((a) => attentionRow(a, data.value as Overview)) : []))

function phaseText(p: WaPhase): string {
  const t: Record<WaPhase, string> = {
    off: L('Gateway nonaktif', 'Gateway off'),
    starting: L('Memulai…', 'Starting…'),
    pairing: L('Menunggu penautan', 'Waiting to be linked'),
    online: 'Online',
    retrying: L('Mencoba ulang', 'Retrying'),
    logged_out: L('Keluar dari WhatsApp', 'Logged out of WhatsApp'),
  }
  return t[p]
}

interface StatusRow {
  icon: string
  kind: string
  k: string
  v: string
  s: string
  to: string
}

const status = computed<StatusRow[]>(() => {
  const o = data.value
  if (!o) return []
  const tg = o.telegram
  const wa = o.whatsapp
  const mm = o.main_model
  const paused = o.search.paused
  const up = uptimeNow.value ?? o.system.uptime_secs
  return [
    { icon: 'power', kind: 'ok', k: 'Daemon', v: L('Berjalan ', 'Running for ') + fmtDuration(up), s: `v${o.system.version}, PID ${o.system.pid}, ${o.system.host}`, to: '/system' },
    {
      icon: 'send',
      kind: tg.online ? 'ok' : tg.configured ? 'err' : '',
      k: 'Telegram',
      v: tg.username ? `@${tg.username}` : L('Belum diatur', 'Not set up'),
      s: tg.online
        ? `Online, owner ${tg.owner_id ?? '—'}, poll ${tg.last_poll_secs === null ? L('belum ada', 'none yet') : fmtAgoSecs(tg.last_poll_secs)}`
        : tg.configured
          ? L('Offline, polling tidak berjalan', 'Offline, polling is not running')
          : L('Token bot dan owner belum diisi', 'The bot token and owner are not set'),
      to: '/telegram',
    },
    {
      icon: 'phone',
      kind: wa.linked && wa.phase === 'online' ? 'ok' : wa.phase === 'retrying' ? 'warn' : '',
      k: 'WhatsApp',
      v: wa.linked ? L('Tertaut', 'Linked') : L('Belum ditautkan', 'Not linked'),
      s: phaseText(wa.phase),
      to: '/whatsapp',
    },
    {
      icon: 'cpu',
      kind: mm.model ? 'info' : 'err',
      k: L('Model utama', 'Main model'),
      v: mm.model ?? L('Belum dipilih', 'Not chosen'),
      s: mm.provider
        ? L(`${mm.provider}, ${nf(mm.catalogue)} model di katalog`, `${mm.provider}, ${nf(mm.catalogue)} models in the catalogue`)
        : L('Tambahkan provider di halaman AI', 'Add a provider on the AI page'),
      to: '/ai',
    },
    {
      icon: 'search',
      kind: paused.length ? 'warn' : 'ok',
      k: L('Pencarian', 'Search'),
      v: o.search.first ?? L('Tidak ada', 'None'),
      s: paused.length ? L(`${listJoin(paused)} sedang dijeda`, `${listJoin(paused)} paused`) : L('Semua mesin siap', 'Every engine is ready'),
      to: '/search',
    },
    {
      icon: 'db',
      kind: '',
      k: L('Penyimpanan', 'Storage'),
      v: `${fmtBytes(o.storage.db_bytes)} database`,
      s: L(`Lampiran ${fmtBytes(o.storage.attachments_bytes)}, ${nf(o.storage.memories)} memori`, `Attachments ${fmtBytes(o.storage.attachments_bytes)}, ${nf(o.storage.memories)} memories`),
      to: '/system',
    },
  ]
})

async function testMain(): Promise<void> {
  const r = await api.post<RoleTestResult>('/api/ai/test/main')
  if (r.ok) toast(L(`${r.model ?? 'Model'} menjawab dalam ${fmtMs(r.ms)}`, `${r.model ?? 'The model'} answered in ${fmtMs(r.ms)}`))
  else toast(r.detail || L('Model utama tidak menjawab', 'The main model did not answer'), true)
}

async function checkTelegram(): Promise<void> {
  const r = await api.post<TelegramCheck>('/api/telegram/check')
  if (r.ok && r.bot) toast(L('Telegram terhubung sebagai @', 'Telegram connected as @') + r.bot.username)
  else toast(r.error ?? L('Telegram tidak menjawab', 'Telegram did not answer'), true)
  void reload(true)
}
</script>

<template>
  <PageHead :title="L('Beranda', 'Home')">
    <template v-if="data">
      <template v-if="lang === 'en'">
        <b :class="{ err: data.queue.failed }">{{ nf(data.queue.failed) }} failed</b> and <b>{{ nf(data.queue.pending) }}</b> waiting in the
        queue. <b>{{ nf(data.storage.messages) }}</b> messages stored across {{ nf(data.storage.conversations) }} conversations,
        <b>{{ nf(data.storage.memories) }}</b> facts in memory.
      </template>
      <template v-else>
        <b :class="{ err: data.queue.failed }">{{ nf(data.queue.failed) }} gagal</b> dan <b>{{ nf(data.queue.pending) }}</b> menunggu di
        antrean. <b>{{ nf(data.storage.messages) }}</b> pesan tersimpan di {{ nf(data.storage.conversations) }} percakapan,
        <b>{{ nf(data.storage.memories) }}</b> fakta di memori.
      </template>
    </template>
  </PageHead>

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <template v-if="attention.length">
      <SectionTitle :title="L('Perlu perhatian', 'Needs attention')"><Badge kind="warn">{{ attention.length }}</Badge></SectionTitle>
      <div class="card rows">
        <RouterLink v-for="(a, i) in attention" :key="i" class="row link" :to="a.to">
          <span class="row-ic" :class="a.kind"><Icon :name="a.icon" /></span>
          <div class="grow">
            <div class="label">{{ a.title }}</div>
            <div v-if="a.hint" class="hint">{{ a.hint }}</div>
          </div>
          <Icon name="chev" />
        </RouterLink>
      </div>
    </template>

    <SectionTitle title="Status" />
    <div class="card rows">
      <RouterLink v-for="r in status" :key="r.k" class="row link srow" :to="r.to">
        <span class="row-ic" :class="r.kind"><Icon :name="r.icon" /></span>
        <div class="grow">
          <div class="k">{{ r.k }}</div>
          <div class="v">{{ r.v }}</div>
          <div class="hint">{{ r.s }}</div>
        </div>
        <Icon name="chev" />
      </RouterLink>
    </div>

    <SectionTitle :title="L('Aksi cepat', 'Quick actions')" />
    <div class="actions">
      <RouterLink class="btn primary" to="/chat"><Icon name="chat" size="sm" />{{ L('Chat dengan Xiao', 'Chat with Xiao') }}</RouterLink>
      <BusyButton class="btn" :run="testMain" :label="L('Menguji…', 'Testing…')"><Icon name="play" size="sm" />{{ L('Uji model utama', 'Test main model') }}</BusyButton>
      <BusyButton class="btn" :run="checkTelegram" :label="L('Memeriksa…', 'Checking…')"><Icon name="send" size="sm" />{{ L('Periksa Telegram', 'Check Telegram') }}</BusyButton>
      <RouterLink class="btn" to="/search"><Icon name="search" size="sm" />{{ L('Uji pencarian', 'Test search') }}</RouterLink>
      <button type="button" class="btn danger" @click="confirmRestart"><Icon name="power" size="sm" />Restart daemon</button>
    </div>
  </template>
</template>
