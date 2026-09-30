<script setup lang="ts">
import { computed } from 'vue'
import { api } from '../api/client'
import type { AttentionItem, Overview, RoleTestResult, TelegramCheck } from '../api/types'
import ActivityChart from '../components/ActivityChart.vue'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import { useLoad } from '../composables/useLoad'
import { fmtAgoSecs, fmtBytes, fmtMs, listJoin } from '../format'
import { L, nf } from '../i18n'
import { confirmRestart } from '../lib/actions'
import { COUNTED_STEPS, countedDone, nextStep, setupSteps } from '../lib/setup'
import { telegramStatus, waPhaseKind, waPhaseText, type StatusKind } from '../lib/status'
import { applyOverview, uptimeNow } from '../stores/session'
import { toast } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<Overview>('/api/overview'), {
  pollMs: 15_000,
  onData: applyOverview,
})

/* ---------- 1. Attention, errors first ---------- */

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

const KIND_ORDER: Record<AttentionItem['kind'], number> = { err: 0, warn: 1, info: 2 }
const attention = computed(() => {
  const o = data.value
  if (!o) return []
  return [...o.attention].sort((a, b) => KIND_ORDER[a.kind] - KIND_ORDER[b.kind]).map((a) => attentionRow(a, o))
})
const attnKind = computed<'err' | 'warn' | 'info'>(() => attention.value[0]?.kind ?? 'info')

/* ---------- 2. Quickstart card (until the essentials are done) ---------- */

const steps = computed(() => (data.value ? setupSteps(data.value) : []))
const stepsDone = computed(() => countedDone(steps.value))
const next = computed(() => nextStep(steps.value))
const stepsPct = computed(() => `${Math.round((stepsDone.value / COUNTED_STEPS) * 100)}%`)

/* ---------- 3. Status tiles ---------- */

interface Tile {
  key: string
  icon: string
  label: string
  kind: StatusKind
  value: string
  sub: string
  to: string
  mono?: boolean
  ticking?: boolean
}

/* A clock that visibly ticks: "02:14:05", or "3h 04:12:09" / "3d 04:12:09" past a day. */
function uptimeClock(total: number): string {
  const s = Math.max(0, Math.floor(total))
  const d = Math.floor(s / 86400)
  const hh = String(Math.floor((s % 86400) / 3600)).padStart(2, '0')
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0')
  const ss = String(s % 60).padStart(2, '0')
  const hms = `${hh}:${mm}:${ss}`
  return d ? `${L(`${d}h`, `${d}d`)} ${hms}` : hms
}

const tiles = computed<Tile[]>(() => {
  const o = data.value
  if (!o) return []
  const tg = o.telegram
  const tgs = telegramStatus(tg.running, tg.online, !!tg.username)
  const tgOff = !tg.configured && !tg.running
  const wa = o.whatsapp
  const mm = o.main_model
  const up = uptimeNow.value ?? o.system.uptime_secs
  return [
    {
      key: 'telegram',
      icon: 'send',
      label: 'Telegram',
      kind: tgOff ? '' : tgs.kind,
      value: tgOff ? L('Belum diatur', 'Not set up') : tg.running ? tgs.text : tg.username ? L('Belum berjalan', 'Not running yet') : L('Mati', 'Off'),
      sub: tg.username ? `@${tg.username}` : tgOff ? L('Isi token dan owner', 'Set the token and owner') : '—',
      to: '/telegram',
    },
    {
      key: 'whatsapp',
      icon: 'phone',
      label: 'WhatsApp',
      kind: waPhaseKind(wa.phase, wa.linked),
      value: wa.linked ? waPhaseText(wa.phase) : wa.phase === 'pairing' ? waPhaseText(wa.phase) : L('Belum ditautkan', 'Not linked'),
      sub: wa.linked ? L('Tertaut', 'Linked') : wa.enabled ? L('Aktif, belum tertaut', 'On, not linked') : L('Gateway mati', 'Gateway off'),
      to: '/whatsapp',
    },
    {
      key: 'model',
      icon: 'cpu',
      label: L('Model utama', 'Main model'),
      kind: mm.model ? '' : 'err',
      value: mm.model ?? L('Belum dipilih', 'Not chosen'),
      sub: mm.provider ?? L('Tambahkan provider', 'Add a provider'),
      to: '/ai',
      mono: !!mm.model,
    },
    {
      key: 'daemon',
      icon: 'clock',
      label: L('Daemon berjalan', 'Daemon uptime'),
      kind: 'ok',
      value: uptimeClock(up),
      sub: `v${o.system.version} · PID ${o.system.pid}`,
      to: '/system',
      ticking: true,
    },
  ]
})

/* ---------- 4. Activity ---------- */

const activity = computed(() => data.value?.activity ?? [])
const today = computed(() => activity.value[activity.value.length - 1] ?? null)
const split = computed(() => {
  const tg = activity.value.reduce((n, d) => n + d.telegram, 0)
  const wa = activity.value.reduce((n, d) => n + d.whatsapp, 0)
  const all = tg + wa
  const share = (v: number): string => (all ? `${Math.round((v / all) * 100)}%` : '—')
  return { tg, wa, tgPct: share(tg), waPct: share(wa) }
})

/* ---------- 5. Quick actions ---------- */

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

/* ---------- 6. Details ---------- */

const lastPoll = computed(() => {
  const s = data.value?.telegram.last_poll_secs
  return s === null || s === undefined ? null : fmtAgoSecs(s)
})
</script>

<template>
  <PageHead :title="L('Beranda', 'Home')" :text="L('Keadaan Xiao sekarang dan 7 hari terakhir.', 'How Xiao is doing now and over the last 7 days.')" />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <!-- 1. Needs attention -->
    <template v-if="attention.length">
      <SectionTitle :title="L('Perlu perhatian', 'Needs attention')">
        <Badge :kind="attnKind">{{ attention.length }}</Badge>
      </SectionTitle>
      <div class="card rows">
        <RouterLink v-for="(a, i) in attention" :key="i" class="row link" :to="a.to">
          <span class="row-ic" :class="a.kind"><Icon :name="a.kind === 'err' ? 'alert' : a.icon" /></span>
          <div class="grow">
            <div class="label">{{ a.title }}</div>
            <div v-if="a.hint" class="hint">{{ a.hint }}</div>
          </div>
          <Icon name="chev" />
        </RouterLink>
      </div>
    </template>

    <!-- 2. Quickstart -->
    <div v-if="!data.setup.complete" class="card card-body qs-card" :class="{ 'mt16': attention.length }">
      <div class="flex top">
        <span class="row-ic info"><Icon name="steps" /></span>
        <div class="grow">
          <div class="label">{{ L('Penyiapan belum selesai', 'Setup is not finished') }}</div>
          <div class="hint">
            <b>{{ stepsDone }}</b>{{ L(` dari ${COUNTED_STEPS} langkah penting selesai.`, ` of ${COUNTED_STEPS} key steps done.`) }}{{ next ? L(` Berikutnya: ${next.title}.`, ` Next: ${next.title}.`) : '' }}
          </div>
        </div>
      </div>
      <div class="meter mt12" role="progressbar" :aria-valuenow="stepsDone" aria-valuemin="0" :aria-valuemax="COUNTED_STEPS" :aria-label="L('Kemajuan penyiapan', 'Setup progress')">
        <i :style="{ width: stepsPct }"></i>
      </div>
      <div class="actions mt12">
        <RouterLink v-if="next" class="btn primary sm" :to="next.to"><Icon :name="next.icon" size="sm" />{{ next.action }}</RouterLink>
        <RouterLink class="btn ghost sm" to="/quickstart">{{ L('Semua langkah', 'All steps') }}<Icon name="chev" size="sm" /></RouterLink>
      </div>
    </div>

    <!-- 3. Status tiles -->
    <SectionTitle title="Status" />
    <div class="tiles-wrap">
      <div class="tiles">
        <RouterLink v-for="t in tiles" :key="t.key" class="tile" :to="t.to">
          <span class="tile-k"><Icon :name="t.icon" size="sm" />{{ t.label }}</span>
          <span class="tile-v" :class="{ mono: t.mono, tick: t.ticking }" :title="t.value">
            <span v-if="t.kind" class="dot" :class="t.kind" aria-hidden="true"></span><span class="tv">{{ t.value }}</span>
          </span>
          <span class="tile-s" :title="t.sub">{{ t.sub }}</span>
        </RouterLink>
      </div>
    </div>

    <!-- 4. Activity -->
    <SectionTitle :title="L('Aktivitas 7 hari', 'Activity, 7 days')" />
    <div class="card">
      <div class="card-body">
        <ActivityChart v-if="activity.length" :days="activity" />
        <div v-else class="small muted">{{ L('Belum ada data aktivitas.', 'No activity data yet.') }}</div>
      </div>
      <dl v-if="activity.length" class="qstats act-stats">
        <div>
          <dt>{{ L('Pesan hari ini', 'Messages today') }}</dt>
          <dd>{{ nf(today?.prompts ?? 0) }}</dd>
        </div>
        <div>
          <dt>{{ L('Jawaban hari ini', 'Answers today') }}</dt>
          <dd>{{ nf(today?.answers ?? 0) }}</dd>
        </div>
        <div>
          <dt>{{ L('Telegram, 7 hari', 'Telegram, 7 days') }}</dt>
          <dd>{{ nf(split.tg) }} <span class="pct">{{ split.tgPct }}</span></dd>
        </div>
        <div>
          <dt>{{ L('WhatsApp, 7 hari', 'WhatsApp, 7 days') }}</dt>
          <dd>{{ nf(split.wa) }} <span class="pct">{{ split.waPct }}</span></dd>
        </div>
      </dl>
    </div>

    <!-- 5. Quick actions -->
    <SectionTitle :title="L('Aksi cepat', 'Quick actions')" />
    <div class="actions">
      <RouterLink class="btn primary" to="/chat"><Icon name="chat" size="sm" />{{ L('Chat dengan Xiao', 'Chat with Xiao') }}</RouterLink>
      <BusyButton class="btn" :run="testMain" :label="L('Menguji…', 'Testing…')"><Icon name="play" size="sm" />{{ L('Uji model utama', 'Test main model') }}</BusyButton>
      <BusyButton class="btn" :run="checkTelegram" :disabled="!data.telegram.configured" :label="L('Memeriksa…', 'Checking…')">
        <Icon name="send" size="sm" />{{ L('Periksa Telegram', 'Check Telegram') }}
      </BusyButton>
      <RouterLink class="btn" to="/search"><Icon name="search" size="sm" />{{ L('Uji pencarian', 'Test search') }}</RouterLink>
      <RouterLink class="btn" to="/quickstart"><Icon name="steps" size="sm" />{{ L('Mulai cepat', 'Quickstart') }}</RouterLink>
      <button type="button" class="btn danger" @click="confirmRestart"><Icon name="power" size="sm" />Restart daemon</button>
    </div>

    <!-- 6. Details -->
    <SectionTitle :title="L('Rincian', 'Details')" />
    <div class="card rows details">
      <RouterLink class="row link srow" to="/queue">
        <span class="row-ic" :class="{ err: data.queue.failed }"><Icon name="inbox" /></span>
        <div class="grow">
          <div class="k">{{ L('Antrean', 'Queue') }}</div>
          <div class="v">
            {{ L(`${nf(data.queue.pending)} menunggu`, `${nf(data.queue.pending)} waiting`) }},
            <span :class="{ 'err-text': data.queue.failed }">{{ L(`${nf(data.queue.failed)} gagal`, `${nf(data.queue.failed)} failed`) }}</span>
          </div>
          <div class="hint">
            {{ lastPoll ? L(`Poll Telegram terakhir ${lastPoll}`, `Last Telegram poll ${lastPoll}`) : L('Belum ada poll Telegram', 'No Telegram poll yet') }}
          </div>
        </div>
        <Icon name="chev" />
      </RouterLink>
      <RouterLink class="row link srow" to="/search">
        <span class="row-ic" :class="{ warn: data.search.paused.length }"><Icon name="search" /></span>
        <div class="grow">
          <div class="k">{{ L('Pencarian', 'Search') }}</div>
          <div class="v">{{ data.search.first ? L(`Pertama: ${data.search.first}`, `First: ${data.search.first}`) : L('Tidak ada mesin aktif', 'No active engine') }}</div>
          <div class="hint">
            {{ data.search.paused.length ? L(`${listJoin(data.search.paused)} sedang dijeda`, `${listJoin(data.search.paused)} paused`) : L('Tidak ada yang dijeda', 'Nothing paused') }},
            {{ L(`${nf(data.search.keyed)} kunci API`, `${nf(data.search.keyed)} API ${data.search.keyed === 1 ? 'key' : 'keys'}`) }}
          </div>
        </div>
        <Icon name="chev" />
      </RouterLink>
      <RouterLink class="row link srow" to="/system">
        <span class="row-ic"><Icon name="db" /></span>
        <div class="grow">
          <div class="k">{{ L('Penyimpanan', 'Storage') }}</div>
          <div class="v">{{ L(`Database ${fmtBytes(data.storage.db_bytes)}, lampiran ${fmtBytes(data.storage.attachments_bytes)}`, `Database ${fmtBytes(data.storage.db_bytes)}, attachments ${fmtBytes(data.storage.attachments_bytes)}`) }}</div>
          <div class="hint">
            {{
              L(
                `${nf(data.storage.memories)} memori, ${nf(data.storage.messages)} pesan di ${nf(data.storage.conversations)} percakapan`,
                `${nf(data.storage.memories)} memories, ${nf(data.storage.messages)} messages in ${nf(data.storage.conversations)} conversations`,
              )
            }}
          </div>
        </div>
        <Icon name="chev" />
      </RouterLink>
    </div>
  </template>
</template>
