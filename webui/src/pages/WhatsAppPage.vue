<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '../api/client'
import type { Ok, PairingView, PairRequest, SettingsRequest, WhatsAppState, WriteResult } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import ChanSwitch from '../components/ChanSwitch.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import Rich from '../components/Rich'
import SectionTitle from '../components/SectionTitle.vue'
import Seg from '../components/Seg.vue'
import Toggle from '../components/Toggle.vue'
import { useForm } from '../composables/useForm'
import { useInterval, useLoad } from '../composables/useLoad'
import { now } from '../format'
import { L, lang, nf } from '../i18n'
import { clock, waPhaseText as phaseText } from '../lib/status'
import { confirmAction, isDesk, toast, useSaveBar } from '../stores/ui'

/* Pairing: a code (default on phones) or a QR code (default on desktop). */
type Mode = 'code' | 'qr'
const mode = ref<Mode>(isDesk() ? 'qr' : 'code')
const phone = ref('')
const modeOptions = computed<{ value: Mode; label: string }[]>(() => [
  { value: 'code', label: L('Kode pairing', 'Pairing code') },
  { value: 'qr', label: L('Kode QR', 'QR code') },
])

const pairing = ref<PairingView | null>(null)
/** Wall-clock time the unfinished pairing stops by itself. */
const stopsAt = ref<number | null>(null)
/** False when the daemon started the pairing by itself (or another tab did). */
const startedHere = ref(false)
/** Why the last pairing ended without linking; shown above the start buttons. */
const ended = ref<string | null>(null)

/* An unfinished pairing that ran out of time, or failed with nothing left to show, is over. */
function isOver(p: PairingView): boolean {
  if (p.done) return false
  if (p.stops_in !== null && p.stops_in <= 0) return true
  return !!p.error && !p.qr && !p.code
}

function track(p: PairingView): void {
  pairing.value = p
  mode.value = p.mode
  stopsAt.value = p.stops_in === null ? null : Date.now() + p.stops_in * 1000
}

const poller = useInterval(() => void pollPairing(), 2000)

/* Polled every 10 s so a pairing the daemon starts by itself (or a phase change) shows up. */
const { data, loading, error, reload } = useLoad(() => api.get<WhatsAppState>('/api/whatsapp'), {
  pollMs: 10_000,
  onData: (d) => {
    /* The 2 s pairing poll owns an active pairing; this only picks up one we do not know yet. */
    if (!pairing.value && d.pairing && !d.pairing.done && !isOver(d.pairing)) {
      ended.value = null
      startedHere.value = false
      track(d.pairing)
      poller.start()
    }
  },
})

interface WaForm {
  enabled: boolean
  owner: string
  groups: string
}
const { form, dirty, reset, initial } = useForm<WaForm>({ enabled: false, owner: '', groups: '' })
const fromState = (d: WhatsAppState): WaForm => ({ enabled: d.enabled, owner: d.owner_number, groups: d.dedicated_groups })
watch(data, (d) => {
  if (d && !dirty.value) reset(fromState(d))
})

const locks = computed(() => data.value?.env_locks ?? {})

async function save(): Promise<void> {
  const was = initial()
  const body: SettingsRequest = {}
  if (form.enabled !== was.enabled) body.WHATSAPP_ENABLED = form.enabled ? 'true' : 'false'
  if (form.owner !== was.owner) {
    const n = form.owner.replace(/\D/g, '')
    if (n && (n.length < 8 || n.length > 15)) throw new Error(L('Nomor harus 8 sampai 15 digit, format internasional', 'The number needs 8 to 15 digits, in international format'))
    body.WHATSAPP_OWNER_NUMBER = n
  }
  if (form.groups !== was.groups) {
    body.WHATSAPP_DEDICATED_GROUPS = form.groups
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean)
      .join(',')
  }
  let r: WriteResult | null = null
  if (Object.keys(body).length) r = await api.put<WriteResult>('/api/settings', body)
  toast(r?.gateway_restarted ? L('Disimpan. Gateway WhatsApp dimulai ulang.', 'Saved. The WhatsApp gateway restarted.') : L('Pengaturan disimpan', 'Settings saved'))
  const d = await reload(true)
  if (d) reset(fromState(d))
}

useSaveBar({ dirty, save, discard: () => data.value && reset(fromState(data.value)) })

function onOwner(e: Event): void {
  const el = e.target as HTMLInputElement
  const digits = el.value.replace(/\D/g, '')
  if (digits !== el.value) el.value = digits
  form.owner = digits
}

/* Any unfinished pairing, whichever mode and whoever started it. */
const active = computed(() => (pairing.value && !pairing.value.done ? pairing.value : null))
const stopsLeft = computed(() => (stopsAt.value === null ? null : Math.max(0, (stopsAt.value - now.value) / 1000)))

async function finishLinked(): Promise<void> {
  toast(L('WhatsApp tertaut dan gateway berjalan.', 'WhatsApp is linked and the gateway is running.'))
  const d = await reload(true)
  if (d) reset(fromState(d))
}

/* The pairing stopped without linking: back to the start buttons with the reason. */
async function endPairing(reason: string | null): Promise<void> {
  poller.stop()
  const last = pairing.value
  pairing.value = null
  stopsAt.value = null
  const d = await reload(true)
  if (d?.linked) {
    ended.value = null
    await finishLinked()
    return
  }
  ended.value =
    reason ??
    last?.error ??
    d?.last_error ??
    L('Penautan berhenti sebelum tertaut. Mulai lagi bila perlu.', 'Linking stopped before it finished. Start again if needed.')
}

async function pollPairing(): Promise<void> {
  let p: PairingView | null
  try {
    p = await api.get<PairingView | null>('/api/whatsapp/pair')
  } catch {
    return /* keep polling; a restart or a hiccup should not end the pairing view */
  }
  if (!pairing.value) return /* cancelled meanwhile */
  if (!p || isOver(p)) {
    await endPairing(p?.error ?? null)
    return
  }
  track(p)
  if (p.done) {
    poller.stop()
    pairing.value = null
    stopsAt.value = null
    await finishLinked()
  }
}

async function startPair(m: Mode): Promise<void> {
  const body: PairRequest = { mode: m }
  if (m === 'code') {
    const num = phone.value.replace(/\D/g, '')
    if (num.length < 8 || num.length > 15) {
      toast(L('Nomor harus 8 sampai 15 digit, format internasional', 'The number needs 8 to 15 digits, in international format'), true)
      return
    }
    body.phone = num
  }
  const p = await api.post<PairingView>('/api/whatsapp/pair', body)
  ended.value = null
  startedHere.value = true
  track(p)
  poller.start()
}

async function cancelPair(): Promise<void> {
  await api.post<Ok>('/api/whatsapp/pair/cancel')
  pairing.value = null
  stopsAt.value = null
  ended.value = null
  poller.stop()
  toast(L('Penautan dibatalkan', 'Linking cancelled'))
  void reload(true)
}

function unlink(): void {
  confirmAction({
    title: L('Putuskan tautan WhatsApp?', 'Unlink WhatsApp?'),
    text: L(
      'Sesi di whatsapp.db dihapus dan Xiao keluar dari perangkat tertaut. Untuk memakai lagi, tautkan ulang.',
      'The session in whatsapp.db is deleted and Xiao leaves the linked devices. Link again to use it.',
    ),
    label: L('Putuskan', 'Unlink'),
    run: async () => {
      await api.post<Ok>('/api/whatsapp/unlink')
      toast(L('Tautan WhatsApp diputus', 'WhatsApp unlinked'))
      const d = await reload(true)
      if (d) reset(fromState(d))
    },
  })
}
</script>

<template>
  <ChanSwitch on="whatsapp" />
  <PageHead
    title="WhatsApp"
    :text="L('Tautkan Xiao sebagai perangkat WhatsApp, lalu atur owner dan grup.', 'Link Xiao as a WhatsApp device, then set the owner and groups.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <div class="card">
      <div class="card-head">
        <span class="row-ic" :class="{ ok: data.linked && data.phase === 'online', warn: data.phase === 'retrying' }"><Icon name="phone" /></span>
        <div class="grow">
          <h4>
            <template v-if="data.linked">
              {{ L('Tertaut', 'Linked') }}
              <Badge v-if="data.phase === 'online'" kind="ok">Online</Badge>
              <Badge v-else-if="data.phase === 'retrying'" kind="warn">{{ L('Mencoba ulang', 'Retrying') }}</Badge>
              <Badge v-else>{{ phaseText(data.phase) }}</Badge>
            </template>
            <template v-else>
              {{ L('Belum ditautkan', 'Not linked') }}
              <Badge v-if="active || data.phase === 'pairing'" kind="info">{{ L('Menunggu penautan', 'Waiting to be linked') }}</Badge>
            </template>
          </h4>
          <div class="sub">
            <template v-if="data.phase === 'retrying' && data.last_error">{{ data.last_error }}</template>
            <template v-else-if="data.linked">{{ L('Gateway berjalan dan diawasi. Restart otomatis bila putus.', 'The gateway runs under supervision and restarts if it drops.') }}</template>
            <template v-else>{{ L('Sesi akan disimpan di whatsapp.db (izin 0600)', 'The session will be stored in whatsapp.db (mode 0600)') }}</template>
          </div>
        </div>
        <label class="flex small" title="WHATSAPP_ENABLED">
          {{ L('Aktif', 'On') }}
          <Toggle v-model="form.enabled" :disabled="!!locks.WHATSAPP_ENABLED" />
        </label>
      </div>
      <div v-if="locks.WHATSAPP_ENABLED" class="card-body"><EnvLock :src="locks.WHATSAPP_ENABLED" /></div>
    </div>

    <template v-if="data.linked">
      <SectionTitle :title="L('Sesi', 'Session')" />
      <div class="card card-body">
        <dl class="kv">
          <dt>{{ L('Status', 'Status') }}</dt>
          <dd>{{ phaseText(data.phase) }}</dd>
          <template v-if="data.last_error">
            <dt>{{ L('Galat terakhir', 'Last error') }}</dt>
            <dd class="err-text">{{ data.last_error }}</dd>
          </template>
          <dt>{{ L('Antrean', 'Queue') }}</dt>
          <dd>{{ L(`${nf(data.queue.pending)} menunggu, ${nf(data.queue.failed)} gagal`, `${nf(data.queue.pending)} pending, ${nf(data.queue.failed)} failed`) }}</dd>
        </dl>
        <div class="actions mt16">
          <button type="button" class="btn danger" @click="unlink"><Icon name="unlink" size="sm" />{{ L('Putuskan tautan', 'Unlink') }}</button>
        </div>
      </div>
    </template>

    <template v-else>
      <SectionTitle :title="L('Tautkan', 'Link')" />
      <div class="card card-body">
        <template v-if="active">
          <div v-if="!startedHere" class="note mb16">
            <Icon name="info" size="sm" />
            <div>{{ L('Penautan ini dimulai oleh daemon, bukan dari halaman ini. Batalkan bila tidak dipakai.', 'The daemon started this linking by itself, not this page. Cancel it if you do not need it.') }}</div>
          </div>

          <template v-if="active.mode === 'code'">
            <div class="pair-code" aria-live="polite">{{ active.code ?? '…' }}</div>
            <ol class="steps small muted mt12">
              <li>
                <template v-if="active.phone">
                  <template v-if="lang === 'en'">On the phone with <b>+{{ active.phone }}</b>, open WhatsApp, then <b>Linked devices</b> and <b>Link a device</b>.</template>
                  <template v-else>Di HP dengan nomor <b>+{{ active.phone }}</b>, buka WhatsApp, lalu <b>Perangkat tertaut</b> dan <b>Tautkan perangkat</b>.</template>
                </template>
                <template v-else>
                  <template v-if="lang === 'en'">On the phone to link, open WhatsApp, then <b>Linked devices</b> and <b>Link a device</b>.</template>
                  <template v-else>Di HP yang akan ditautkan, buka WhatsApp, lalu <b>Perangkat tertaut</b> dan <b>Tautkan perangkat</b>.</template>
                </template>
              </li>
              <li>
                <template v-if="lang === 'en'">Choose <b>Link with phone number instead</b>, then enter the code above.</template>
                <template v-else>Pilih <b>Tautkan dengan nomor telepon saja</b>, lalu masukkan kode di atas.</template>
              </li>
            </ol>
          </template>

          <template v-else>
            <svg
              v-if="active.qr"
              class="qr"
              :viewBox="`-2 -2 ${active.qr.size + 4} ${active.qr.size + 4}`"
              shape-rendering="crispEdges"
              role="img"
              :aria-label="L('Kode QR WhatsApp', 'WhatsApp QR code')"
            >
              <path fill="currentColor" :d="active.qr.path" />
            </svg>
            <div v-else class="loading sm"><span class="spin-inline" aria-hidden="true"></span>{{ L('Menyiapkan kode QR…', 'Preparing the QR code…') }}</div>
            <ol class="steps small muted mt12">
              <li>
                <template v-if="lang === 'en'">On your phone, open WhatsApp, then <b>Linked devices</b> and <b>Link a device</b>.</template>
                <template v-else>Di HP, buka WhatsApp, lalu <b>Perangkat tertaut</b> dan <b>Tautkan perangkat</b>.</template>
              </li>
              <li>{{ L('Pindai kode ini. Kode diganti setiap 20 detik.', 'Scan this code. It changes every 20 seconds.') }}</li>
            </ol>
            <div class="note warn mt12">
              <Icon name="phone" size="sm" />
              <div>
                <template v-if="lang === 'en'">A phone cannot scan a QR code shown on its own screen. On a phone, cancel and use the <b>Pairing code</b>.</template>
                <template v-else>QR tidak bisa dipindai dari HP yang sedang membuka halaman ini. Di HP, batalkan lalu pakai <b>Kode pairing</b>.</template>
              </div>
            </div>
          </template>

          <div v-if="active.error" class="note err mt12" role="alert"><Icon name="alert" size="sm" /><div>{{ active.error }}</div></div>

          <div class="pair-wait mt12">
            <span class="spin-inline" aria-hidden="true"></span>
            <span class="grow">
              {{ L('Menunggu konfirmasi dari WhatsApp…', 'Waiting for WhatsApp to confirm…') }}
              <span v-if="stopsLeft !== null" class="faint nowrap">
                {{ L(`Berhenti otomatis dalam ${clock(stopsLeft)}`, `Stops by itself in ${clock(stopsLeft)}`) }}
              </span>
            </span>
            <BusyButton class="btn sm" :run="cancelPair" :label="L('Membatalkan…', 'Cancelling…')">
              <Icon name="x" size="sm" />{{ L('Batal', 'Cancel') }}
            </BusyButton>
          </div>
        </template>

        <template v-else>
          <div v-if="ended" class="note err mb16" role="alert">
            <Icon name="alert" size="sm" />
            <div><b>{{ L('Penautan berhenti.', 'Linking stopped.') }}</b> {{ ended }}</div>
          </div>
          <Seg v-model="mode" :options="modeOptions" :label="L('Cara menautkan', 'How to link')" />
          <div class="mt16">
            <form v-if="mode === 'code'" class="field mt0" @submit.prevent>
              <label class="lab" for="wa-num">{{ L('Nomor WhatsApp yang akan ditautkan', 'WhatsApp number to link') }}</label>
              <div class="inline">
                <input id="wa-num" v-model="phone" class="input mono" type="tel" inputmode="numeric" autocomplete="tel" placeholder="628123456789" />
                <BusyButton type="submit" class="btn primary" :run="() => startPair('code')" :label="L('Meminta…', 'Requesting…')">
                  {{ L('Minta kode', 'Get code') }}
                </BusyButton>
              </div>
              <div class="help">
                {{ L('Format internasional tanpa “+”. Nomor ini akan menjadi perangkat tempat Xiao berjalan.', 'International format without “+”. Xiao runs as a linked device of this number.') }}
              </div>
            </form>
            <BusyButton v-else class="btn primary" :run="() => startPair('qr')" :label="L('Meminta…', 'Requesting…')">
              <Icon name="qr" size="sm" />{{ L('Tampilkan kode QR', 'Show QR code') }}
            </BusyButton>
          </div>
        </template>

        <div class="help mt12">
          <Rich
            :text="L('Penautan berjalan di dalam daemon, jadi layanan tidak perlu dihentikan (berbeda dengan `xiao gateway wa pair`). Penautan yang belum selesai berhenti sendiri setelah 3 menit.', 'Linking runs inside the daemon, so the service keeps running (unlike `xiao gateway wa pair`). An unfinished linking stops by itself after 3 minutes.')"
          />
        </div>
      </div>
    </template>

    <SectionTitle :title="L('Akses', 'Access')"><EffectBadge type="gateway" /></SectionTitle>
    <div class="card card-body">
      <label class="field">
        <span class="lab">{{ L('Nomor owner', 'Owner number') }}</span>
        <input :value="form.owner" class="input mono" type="tel" inputmode="numeric" placeholder="628123456789" :disabled="!!locks.WHATSAPP_OWNER_NUMBER" @input="onOwner" />
        <span class="help">
          <code>WHATSAPP_OWNER_NUMBER</code>
          {{ L('hanya nomor ini yang dilayani. Kosong = semua pesan dibuang.', 'only this number is served. Empty = every message is dropped.') }}
        </span>
      </label>
      <EnvLock :src="locks.WHATSAPP_OWNER_NUMBER" />
      <label class="field">
        <span class="lab">{{ L('Grup khusus', 'Dedicated groups') }}</span>
        <input v-model="form.groups" class="input mono" autocomplete="off" placeholder="120363000000000000@g.us" :disabled="!!locks.WHATSAPP_DEDICATED_GROUPS" />
        <span class="help">
          <code>WHATSAPP_DEDICATED_GROUPS</code>
          {{ L('di grup lain Xiao hanya menjawab bila disebut, di-reply, atau pesan diawali “/”.', 'in other groups Xiao only answers when mentioned, replied to, or when the message starts with “/”.') }}
        </span>
      </label>
      <EnvLock :src="locks.WHATSAPP_DEDICATED_GROUPS" />
    </div>
  </template>
</template>
