<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '../api/client'
import type { SecretWriteResult, SettingsRequest, TelegramCheck, TelegramState, WriteResult } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import ChanSwitch from '../components/ChanSwitch.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import SecretField from '../components/SecretField.vue'
import { useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { fmtAgoSecs } from '../format'
import { L, nf } from '../i18n'
import { toast, useSaveBar } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<TelegramState>('/api/telegram'))

interface TgForm {
  owner: string
  allowed: string
  dedicated: string
}
const { form, dirty, reset, initial } = useForm<TgForm>({ owner: '', allowed: '', dedicated: '' })
const fromState = (d: TelegramState): TgForm => ({ owner: d.owner_id, allowed: d.allowed_chat_ids, dedicated: d.dedicated_chat_ids })

watch(data, (d) => {
  if (d && !dirty.value) reset(fromState(d))
})

const locks = computed(() => data.value?.env_locks ?? {})
const bot = computed(() => data.value?.bot ?? null)
const lastPoll = computed(() => {
  const s = data.value?.last_poll_secs
  return s === null || s === undefined ? L('belum ada', 'none yet') : fmtAgoSecs(s)
})

const ids = (v: string): string[] =>
  v
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean)

function addId(field: 'allowed' | 'dedicated', id: string): void {
  const list = ids(form[field])
  if (!list.includes(id)) list.push(id)
  form[field] = list.join(', ')
}

const idListOk = (v: string): boolean => ids(v).every((x) => /^-?\d+$/.test(x))

async function save(): Promise<void> {
  const was = initial()
  const body: SettingsRequest = {}
  if (form.owner !== was.owner) {
    if (form.owner && !/^\d+$/.test(form.owner.trim())) throw new Error(L('Owner user ID hanya berisi angka.', 'The owner user ID has digits only.'))
    body.OWNER_USER_ID = form.owner.trim()
  }
  if (form.allowed !== was.allowed) {
    if (!idListOk(form.allowed)) throw new Error(L('ALLOWED_CHAT_IDS berisi ID chat yang dipisah koma.', 'ALLOWED_CHAT_IDS holds chat ids separated by commas.'))
    body.ALLOWED_CHAT_IDS = ids(form.allowed).join(',')
  }
  if (form.dedicated !== was.dedicated) {
    if (!idListOk(form.dedicated)) throw new Error(L('DEDICATED_CHAT_IDS berisi ID chat yang dipisah koma.', 'DEDICATED_CHAT_IDS holds chat ids separated by commas.'))
    body.DEDICATED_CHAT_IDS = ids(form.dedicated).join(',')
  }
  if (Object.keys(body).length) await api.put<WriteResult>('/api/settings', body)
  toast(L('Disimpan. Berlaku setelah restart.', 'Saved. Applies after a restart.'))
  const d = await reload(true)
  if (d) reset(fromState(d))
}

useSaveBar({ dirty, save, discard: () => data.value && reset(fromState(data.value)) })

/* Owner id: digits only */
function onOwner(e: Event): void {
  const el = e.target as HTMLInputElement
  const digits = el.value.replace(/\D/g, '')
  if (digits !== el.value) el.value = digits
  form.owner = digits
}

/* getMe check */
const check = ref<TelegramCheck | null>(null)
async function runCheck(): Promise<void> {
  const r = await api.post<TelegramCheck>('/api/telegram/check')
  check.value = r
  if (r.ok && r.bot) toast(L('Telegram terhubung sebagai @', 'Telegram connected as @') + r.bot.username)
  else toast(r.error ?? L('Telegram tidak menjawab', 'Telegram did not answer'), true)
  void reload(true)
}

const tokenToast = (r: SecretWriteResult): string =>
  r.detail
    ? L(`Token valid untuk ${r.detail}. Berlaku setelah restart.`, `The token is valid for ${r.detail}. It applies after a restart.`)
    : L('Token disimpan. Berlaku setelah restart.', 'Token saved. It applies after a restart.')

type RowState = 'ok' | 'warn' | 'info'
interface CheckRow {
  state: RowState
  title: string
  text: string
  fix?: string
  src?: string
}

const rows = computed<CheckRow[]>(() => {
  const d = data.value
  const b = bot.value
  if (!d || !b) return []
  const out: CheckRow[] = []
  if (b.can_read_all_group_messages === true) {
    out.push({ state: 'ok', title: L('Privacy mode mati', 'Privacy mode is off'), text: L('Bot menerima semua pesan grup.', 'The bot receives every group message.'), src: 'getMe: can_read_all_group_messages' })
  } else if (b.can_read_all_group_messages === false) {
    out.push({
      state: 'warn',
      title: L('Privacy mode masih aktif', 'Privacy mode is still on'),
      text: L(
        `Di grup, bot hanya menerima reply ke bot, pesan berawalan /, dan semua pesan bila bot admin. Sebutan “@${b.username}” biasa tidak sampai.`,
        `In groups the bot only receives replies to it, messages starting with /, and everything when it is an admin. A plain “@${b.username}” mention does not arrive.`,
      ),
      fix: L(
        'Di @BotFather kirim /setprivacy, pilih bot, lalu Disable. Setelah itu keluarkan lalu masukkan lagi bot ke grup, atau jadikan bot admin.',
        'In @BotFather send /setprivacy, pick the bot, then Disable. Then remove the bot from the group and add it again, or make it an admin.',
      ),
      src: 'getMe: can_read_all_group_messages = false',
    })
  }
  if (b.supports_inline_queries === true) {
    out.push({
      state: 'ok',
      title: L('Inline mode aktif', 'Inline mode is on'),
      text: L(`Pertanyaan “@${b.username} …” di chat mana pun dijawab untuk owner.`, `Questions like “@${b.username} …” in any chat are answered for the owner.`),
      src: 'getMe: supports_inline_queries',
    })
  } else if (b.supports_inline_queries === false) {
    out.push({
      state: 'warn',
      title: L('Inline mode mati', 'Inline mode is off'),
      text: L(`Pertanyaan “@${b.username} …” dari chat lain tidak sampai ke bot.`, `Questions like “@${b.username} …” from other chats do not reach the bot.`),
      fix: L('Di @BotFather kirim /setinline, pilih bot, lalu isi teks placeholder.', 'In @BotFather send /setinline, pick the bot, then enter a placeholder text.'),
      src: 'getMe: supports_inline_queries = false',
    })
  }
  const seen = d.inline_queries_seen
  const chosen = d.chosen_results_seen
  const queueSrc = L('riwayat antrean (Telegram tidak menyediakan cek langsung)', 'queue history (Telegram offers no direct check)')
  if (b.supports_inline_queries && seen > 0 && chosen === 0) {
    out.push({
      state: 'warn',
      title: L('Inline feedback kemungkinan belum aktif', 'Inline feedback is probably off'),
      text: L(
        `Belum pernah ada chosen_inline_result yang masuk, padahal inline query sudah ${nf(seen)} kali. Tanpa ini jawaban inline berhenti di placeholder.`,
        `No chosen_inline_result has arrived yet, although there ${seen === 1 ? 'has been 1 inline query' : `have been ${nf(seen)} inline queries`}. Without it, inline answers stop at the placeholder.`,
      ),
      fix: L('Di @BotFather kirim /setinlinefeedback, pilih bot, lalu Enabled.', 'In @BotFather send /setinlinefeedback, pick the bot, then Enabled.'),
      src: queueSrc,
    })
  } else if (chosen > 0) {
    out.push({
      state: 'ok',
      title: L('Inline feedback aktif', 'Inline feedback is on'),
      text: L(`${nf(chosen)} hasil inline dipilih sejak daemon mulai.`, `${nf(chosen)} inline ${chosen === 1 ? 'result' : 'results'} chosen since the daemon started.`),
      src: queueSrc,
    })
  } else {
    out.push({
      state: 'info',
      title: L('Inline feedback belum bisa diperiksa', 'Inline feedback cannot be checked yet'),
      text: L('Belum ada data: coba satu pertanyaan inline dulu.', 'No data yet: try one inline question first.'),
      src: queueSrc,
    })
  }
  if (b.can_join_groups === true) {
    out.push({ state: 'ok', title: L('Bisa dimasukkan ke grup', 'Can join groups'), text: L('Grup baru bisa menambahkan bot.', 'New groups can add the bot.'), src: 'getMe: can_join_groups' })
  } else if (b.can_join_groups === false) {
    out.push({
      state: 'warn',
      title: L('Tidak bisa dimasukkan ke grup', 'Cannot join groups'),
      text: L('Grup baru tidak bisa menambahkan bot.', 'New groups cannot add the bot.'),
      fix: L('Di @BotFather kirim /setjoingroups, pilih bot, lalu Enable.', 'In @BotFather send /setjoingroups, pick the bot, then Enable.'),
      src: 'getMe: can_join_groups = false',
    })
  }
  if (b.supports_guest_queries === true) {
    out.push({ state: 'ok', title: L('Mode guest aktif', 'Guest mode is on'), text: L('Owner bisa memanggil Xiao di chat lain lewat guest query.', 'The owner can call Xiao in other chats with a guest query.'), src: 'getMe: supports_guest_queries' })
  }
  return out
})

const rowIcon = (s: RowState): string => ({ ok: 'check', warn: 'alert', info: 'info' })[s]
</script>

<template>
  <ChanSwitch on="telegram" />
  <PageHead
    title="Telegram"
    :text="L('Token bot, owner, grup yang dilayani, dan pemeriksaan pengaturan @BotFather.', 'Bot token, owner, the groups Xiao serves, and a check of the @BotFather settings.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <div v-if="!data.configured" class="note warn mb16">
      <Icon name="alert" size="sm" />
      <div>
        {{
          L(
            'Telegram belum diatur. Isi token bot dari @BotFather dan owner user ID (akun Telegram Anda) di bawah, lalu restart daemon.',
            'Telegram is not set up yet. Enter the bot token from @BotFather and the owner user ID (your Telegram account) below, then restart the daemon.',
          )
        }}
      </div>
    </div>

    <div class="card">
      <div class="card-head">
        <span class="row-ic" :class="bot ? (data.online ? 'ok' : 'err') : ''"><Icon name="send" /></span>
        <div class="grow">
          <template v-if="bot">
            <h4>
              @{{ bot.username }}
              <Badge v-if="data.online" kind="ok">Online</Badge>
              <Badge v-else kind="err">Offline</Badge>
            </h4>
            <div class="sub">
              {{ bot.first_name }}, ID {{ bot.id }}, {{ L(`long-poll, timeout ${data.poll_timeout} dtk`, `long-poll, ${data.poll_timeout} s timeout`) }}
            </div>
          </template>
          <template v-else>
            <h4>{{ data.token.set ? L('Bot tidak terjangkau', 'The bot cannot be reached') : L('Belum ada token bot', 'No bot token yet') }}</h4>
            <div class="sub">{{ data.bot_error ?? L('Isi token bot di bawah.', 'Set the bot token below.') }}</div>
          </template>
        </div>
        <BusyButton class="btn sm" :run="runCheck" :disabled="!data.token.set" :label="L('Memeriksa…', 'Checking…')">
          <Icon name="refresh" size="sm" />{{ L('Periksa', 'Check') }}
        </BusyButton>
      </div>
      <div class="card-body">
        <template v-if="check">
          <dl v-if="check.ok && check.bot" class="kv">
            <dt>getMe</dt>
            <dd>OK, {{ nf(check.ms) }} ms</dd>
            <dt>Username</dt>
            <dd>@{{ check.bot.username }}</dd>
            <dt>Privacy mode</dt>
            <dd>{{ check.bot.can_read_all_group_messages ? L('mati', 'off') : L('aktif', 'on') }}</dd>
            <dt>Inline</dt>
            <dd>{{ check.bot.supports_inline_queries ? L('aktif', 'on') : L('mati', 'off') }}</dd>
            <dt>Webhook</dt>
            <dd class="break">{{ check.webhook_url || L('tidak ada (long polling)', 'none (long polling)') }}</dd>
            <dt>{{ L('Update tertunda', 'Pending updates') }}</dt>
            <dd>{{ check.pending_updates === null ? '—' : nf(check.pending_updates) }}</dd>
          </dl>
          <div v-else class="note err"><Icon name="alert" size="sm" /><div>{{ check.error ?? L('Telegram tidak menjawab.', 'Telegram did not answer.') }}</div></div>
        </template>
        <div v-else class="small muted">
          {{ L(`Polling terakhir ${lastPoll}. Bot API 10.3. Menu slash dikosongkan saat start.`, `Last poll ${lastPoll}. Bot API 10.3. The slash menu is cleared at start.`) }}
        </div>
      </div>
    </div>

    <SectionTitle :title="L('Akses', 'Access')" />
    <div class="card card-body">
      <div class="field">
        <div class="lab">{{ L('Token bot', 'Bot token') }} <EffectBadge type="secret" /> <EffectBadge type="restart" /></div>
        <SecretField
          secret-key="BOT_TOKEN"
          :meta="data.token"
          :locked="locks.BOT_TOKEN"
          :saved-toast="tokenToast"
          :delete-warn="L('Tanpa token, Telegram berhenti setelah restart sampai token baru diisi.', 'Without a token, Telegram stops after the restart until a new one is set.')"
          @changed="reload(true)"
        />
        <div class="help">
          {{ L('Diperiksa dengan getMe sebelum disimpan. Token baru dipakai setelah daemon di-restart.', 'Checked with getMe before it is saved. The new token is used after the daemon restarts.') }}
        </div>
      </div>
      <label class="field">
        <span class="lab">Owner user ID <EffectBadge type="restart" /></span>
        <input :value="form.owner" class="input mono" inputmode="numeric" autocomplete="off" :disabled="!!locks.OWNER_USER_ID" @input="onOwner" />
        <span class="help">
          <code>OWNER_USER_ID</code>
          {{ L('hanya akun ini yang dilayani; pesan orang lain dibuang tanpa balasan.', 'only this account is served; messages from anyone else are dropped without a reply.') }}
        </span>
      </label>
      <EnvLock :src="locks.OWNER_USER_ID" />
    </div>

    <SectionTitle :title="L('Grup', 'Groups')" />
    <div class="card card-body">
      <label class="field">
        <span class="lab">{{ L('Grup yang diizinkan', 'Allowed groups') }} <EffectBadge type="restart" /></span>
        <input v-model="form.allowed" class="input mono" autocomplete="off" :placeholder="L('Kosong = semua grup', 'Empty = every group')" :disabled="!!locks.ALLOWED_CHAT_IDS" />
        <span class="help">
          <code>ALLOWED_CHAT_IDS</code>
          {{ L('pisahkan dengan koma. Chat pribadi dan grup khusus selalu dilayani.', 'separated by commas. The private chat and dedicated groups are always served.') }}
        </span>
      </label>
      <EnvLock :src="locks.ALLOWED_CHAT_IDS" />
      <label class="field">
        <span class="lab">{{ L('Grup khusus Xiao', 'Dedicated Xiao groups') }} <EffectBadge type="restart" /></span>
        <input v-model="form.dedicated" class="input mono" autocomplete="off" :disabled="!!locks.DEDICATED_CHAT_IDS" />
        <span class="help">
          <code>DEDICATED_CHAT_IDS</code>
          {{ L('setiap pesan owner dijawab tanpa perlu menyebut bot. Forum tempat bot jadi admin otomatis dianggap khusus.', 'every owner message is answered without mentioning the bot. Forums where the bot is an admin count as dedicated on their own.') }}
        </span>
      </label>
      <EnvLock :src="locks.DEDICATED_CHAT_IDS" />
      <template v-if="data.groups_seen.length">
        <div class="small muted mt16">{{ L('Grup yang pernah terlihat di riwayat:', 'Groups seen in the history:') }}</div>
        <div v-for="g in data.groups_seen" :key="g" class="actions mt8">
          <button type="button" class="btn sm" :disabled="ids(form.allowed).includes(g) || !!locks.ALLOWED_CHAT_IDS" @click="addId('allowed', g)">
            <Icon name="plus" size="sm" />{{ L(`Izinkan ${g}`, `Allow ${g}`) }}
          </button>
          <button type="button" class="btn sm" :disabled="ids(form.dedicated).includes(g) || !!locks.DEDICATED_CHAT_IDS" @click="addId('dedicated', g)">
            <Icon name="plus" size="sm" />{{ L(`Jadikan khusus ${g}`, `Make ${g} dedicated`) }}
          </button>
        </div>
      </template>
    </div>

    <template v-if="bot">
      <SectionTitle :title="L('Pemeriksaan @BotFather', '@BotFather check')" />
      <div class="card rows">
        <div v-for="(r, i) in rows" :key="i" class="row top">
          <span class="row-ic" :class="r.state"><Icon :name="rowIcon(r.state)" /></span>
          <div class="grow">
            <div class="label">{{ r.title }}</div>
            <div class="hint">{{ r.text }}</div>
            <div v-if="r.fix" class="hint mt8"><b>{{ L('Cara memperbaiki:', 'How to fix:') }}</b> {{ r.fix }}</div>
            <div v-if="r.src" class="hint faint mt8">{{ L('Sumber:', 'Source:') }} {{ r.src }}</div>
          </div>
        </div>
      </div>
    </template>
  </template>
</template>
