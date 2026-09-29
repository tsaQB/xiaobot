<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api, seg } from '../api/client'
import type { DeviceOs, Ok, SecurityState, SettingsRequest, WebSessionView, WriteResult } from '../api/types'
import Badge from '../components/Badge.vue'
import BusyButton from '../components/BusyButton.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import SecretField from '../components/SecretField.vue'
import Sheet from '../components/Sheet.vue'
import Toggle from '../components/Toggle.vue'
import { intIn, useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { fmtAgo } from '../format'
import { L, lang } from '../i18n'
import { markSignedOut } from '../stores/session'
import { confirmAction, copyText, toast, useSaveBar, useSheet } from '../stores/ui'

const router = useRouter()
const { data, loading, error, reload } = useLoad(() => api.get<SecurityState>('/api/security'))

interface SecForm {
  lan: boolean
  port: string
  networks: string
  tgLogin: boolean
  days: string
}

function fromState(d: SecurityState): SecForm {
  const saved = d.saved_bind || d.bind
  const i = saved.lastIndexOf(':')
  const addr = i > 0 ? saved.slice(0, i) : saved
  const port = i > 0 ? saved.slice(i + 1) : String(d.port)
  return {
    lan: addr === '0.0.0.0' || addr === '::' || addr === '[::]',
    port: /^\d+$/.test(port) ? port : String(d.port),
    networks: d.allowed_networks,
    tgLogin: d.telegram_login,
    days: String(d.session_days),
  }
}

const { form, dirty, reset, initial } = useForm<SecForm>({ lan: false, port: '8787', networks: '', tgLogin: true, days: '7' })
watch(data, (d) => {
  if (d && !dirty.value) reset(fromState(d))
})

const locks = computed(() => data.value?.env_locks ?? {})
const addr = computed(() => (form.lan ? '0.0.0.0' : '127.0.0.1'))
const portNum = computed(() => intIn(form.port, 1024, 65535))
const portInvalid = computed(() => (portNum.value === null ? L('Port harus bilangan bulat 1024 sampai 65535.', 'The port must be a whole number from 1024 to 65535.') : null))
const bindValue = computed(() => `${addr.value}:${portNum.value ?? form.port}`)
const server = computed(() => data.value?.lan_ip ?? '<server>')
const lanUrl = computed(() => `http://${server.value}:${portNum.value ?? form.port}`)

function pick(lan: boolean): void {
  if (!locks.value.XIAO_WEB_BIND) form.lan = lan
}

/* Telegram code sign-in can only be turned off while a password is set. */
const tgDisabledHint = computed(() => {
  const d = data.value
  if (!d) return null
  if (locks.value.XIAO_WEB_TELEGRAM_LOGIN) return null
  if (!d.telegram_available) return L('Telegram belum diatur, jadi kode tidak bisa dikirim.', 'Telegram is not set up, so no code can be sent.')
  if (!d.password.set && form.tgLogin) return L('Pasang kata sandi cadangan dulu sebelum mematikan cara ini.', 'Set a backup password before turning this off.')
  return null
})

async function save(): Promise<void> {
  const was = initial()
  const body: SettingsRequest = {}
  let restartKeys = false
  if (form.lan !== was.lan || form.port !== was.port) {
    if (portNum.value === null) throw new Error(portInvalid.value ?? '')
    body.XIAO_WEB_BIND = `${addr.value}:${portNum.value}`
    restartKeys = true
  }
  if (form.networks !== was.networks) {
    body.XIAO_WEB_ALLOWED_NETWORKS = form.networks
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean)
      .join(', ')
    restartKeys = true
  }
  if (form.tgLogin !== was.tgLogin) body.XIAO_WEB_TELEGRAM_LOGIN = form.tgLogin ? 'true' : 'false'
  if (form.days !== was.days) body.XIAO_WEB_SESSION_DAYS = form.days
  if (Object.keys(body).length) await api.put<WriteResult>('/api/settings', body)
  toast(restartKeys ? L('Disimpan. Berlaku setelah restart.', 'Saved. Applies after a restart.') : L('Pengaturan disimpan', 'Settings saved'))
  const d = await reload(true)
  if (d) reset(fromState(d))
}

useSaveBar({ dirty, save, discard: () => data.value && reset(fromState(data.value)) })

async function copyUrl(): Promise<void> {
  const ok = await copyText(lanUrl.value)
  toast(ok ? L('Alamat disalin', 'Address copied') : L('Tidak bisa menyalin; salin manual.', 'Could not copy; copy it by hand.'), !ok)
}

/* Backup password */
const pwSheet = useSheet()
const pw1 = ref('')
const pw2 = ref('')
const pwError = ref<string | null>(null)

function openPassword(): void {
  pw1.value = ''
  pw2.value = ''
  pwError.value = null
  pwSheet.show()
}

async function savePassword(): Promise<void> {
  if (pw1.value.length < 8) {
    pwError.value = L('Kata sandi paling sedikit 8 karakter.', 'The password needs at least 8 characters.')
    return
  }
  if (pw1.value !== pw2.value) {
    pwError.value = L('Kedua kata sandi tidak sama.', 'The two passwords do not match.')
    return
  }
  await api.put<Ok>('/api/security/password', { password: pw1.value })
  pw1.value = ''
  pw2.value = ''
  pwSheet.hide()
  toast(L('Kata sandi cadangan disimpan', 'Backup password saved'))
  await reload(true)
}

const deletePassword = (): Promise<Ok> => api.del<Ok>('/api/security/password')

/* Devices */
const osIcon = (os: DeviceOs): string => (os === 'android' || os === 'ios' ? 'phone' : 'term')
const others = computed(() => (data.value ? data.value.sessions.filter((s) => !s.current).length : 0))

async function revoke(s: WebSessionView): Promise<void> {
  await api.del<Ok>(`/api/security/sessions/${seg(s.id)}`)
  toast(L('Sesi dicabut', 'Session revoked'))
  await reload(true)
}

async function revokeOthers(): Promise<void> {
  await api.post<Ok>('/api/security/sessions/revoke-others')
  toast(L('Perangkat lain dicabut', 'Other devices revoked'))
  await reload(true)
}

function signOutEverywhere(): void {
  confirmAction({
    title: L('Keluar dari semua perangkat?', 'Sign out everywhere?'),
    text: L('Semua sesi dicabut, termasuk perangkat ini.', 'Every session is revoked, this device included.'),
    label: L('Keluar semua', 'Sign out all'),
    run: async () => {
      await api.post<Ok>('/api/auth/logout-all')
      markSignedOut()
      await router.replace({ name: 'login' })
    },
  })
}

const dayOptions = computed(() => [
  { value: '1', label: L('1 hari', '1 day') },
  { value: '7', label: L('7 hari', '7 days') },
  { value: '30', label: L('30 hari', '30 days') },
])
</script>

<template>
  <PageHead
    :title="L('Keamanan WebUI', 'WebUI security')"
    :text="L('Dari mana konsol ini bisa dibuka, dan bagaimana owner masuk.', 'Where this console can be opened from, and how the owner signs in.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle :title="L('Alamat', 'Address')"><EffectBadge type="restart" /></SectionTitle>
    <div class="grid two">
      <button type="button" class="preset" :class="{ on: !form.lan }" :aria-pressed="!form.lan" :disabled="!!locks.XIAO_WEB_BIND" @click="pick(false)">
        <span class="preset-top">
          <span class="row-ic"><Icon name="lock" /></span><b>{{ L('Hanya server ini', 'This server only') }}</b>
          <span class="preset-check"><Icon name="check" size="sm" /></span>
        </span>
        <span class="mono small">127.0.0.1:{{ portNum ?? form.port }}</span>
        <span class="hint">{{ L('Dibuka di server sendiri, atau dari HP lewat tunnel SSH atau Cloudflare.', 'Opened on the server itself, or from a phone through an SSH or Cloudflare tunnel.') }}</span>
      </button>
      <button type="button" class="preset" :class="{ on: form.lan }" :aria-pressed="form.lan" :disabled="!!locks.XIAO_WEB_BIND" @click="pick(true)">
        <span class="preset-top">
          <span class="row-ic"><Icon name="globe" /></span><b>{{ L('Jaringan lokal (LAN)', 'Local network (LAN)') }}</b>
          <span class="preset-check"><Icon name="check" size="sm" /></span>
        </span>
        <span class="mono small">0.0.0.0:{{ portNum ?? form.port }}</span>
        <span class="hint">{{ L('HP dan laptop di Wi-Fi yang sama bisa membukanya langsung.', 'Phones and laptops on the same Wi-Fi can open it directly.') }}</span>
      </button>
    </div>

    <div class="card card-body mt12">
      <div class="grid two">
        <label class="field mt0">
          <span class="lab">{{ L('Alamat', 'Address') }}</span>
          <input class="input mono" :value="addr" disabled />
        </label>
        <label class="field mt0">
          <span class="lab">Port</span>
          <input
            :value="form.port"
            class="input mono"
            type="number"
            inputmode="numeric"
            min="1024"
            max="65535"
            step="1"
            :disabled="!!locks.XIAO_WEB_BIND"
            :aria-invalid="!!portInvalid"
            @input="form.port = ($event.target as HTMLInputElement).value"
          />
          <span v-if="portInvalid" class="field-err">{{ portInvalid }}</span>
        </label>
      </div>
      <div class="help"><code>XIAO_WEB_BIND</code> = <span class="mono">{{ bindValue }}</span></div>
      <div v-if="data.bind !== data.saved_bind" class="help">
        {{ L(`Sekarang mendengarkan di ${data.bind}; ${data.saved_bind} berlaku setelah restart.`, `Listening on ${data.bind} now; ${data.saved_bind} applies after a restart.`) }}
      </div>
      <EnvLock :src="locks.XIAO_WEB_BIND" />

      <template v-if="form.lan">
        <label class="field">
          <span class="lab">{{ L('Jaringan yang diizinkan', 'Allowed networks') }}</span>
          <input v-model="form.networks" class="input mono" autocomplete="off" spellcheck="false" :disabled="!!locks.XIAO_WEB_ALLOWED_NETWORKS" />
          <span class="help">
            <code>XIAO_WEB_ALLOWED_NETWORKS</code>
            {{ L('alamat di luar daftar ini ditolak sebelum halaman masuk. Loopback selalu diizinkan.', 'addresses outside this list are refused before the sign-in page. Loopback is always allowed.') }}
          </span>
        </label>
        <EnvLock :src="locks.XIAO_WEB_ALLOWED_NETWORKS" />
        <div class="field">
          <div class="lab">{{ L('Buka dari HP atau laptop', 'Open from a phone or laptop') }}</div>
          <div class="inline">
            <input class="input mono" readonly :value="lanUrl" :aria-label="L('Alamat konsol', 'Console address')" />
            <button type="button" class="btn" @click="copyUrl"><Icon name="copy" size="sm" />{{ L('Salin', 'Copy') }}</button>
          </div>
        </div>
        <div class="note warn mt12">
          <Icon name="alert" size="sm" />
          <div>
            {{ L('Lalu lintas HTTP tidak terenkripsi. Pakai mode ini hanya di jaringan rumah yang tepercaya, atau pasang HTTPS di depannya (Caddy atau Cloudflare Tunnel).', 'HTTP traffic is not encrypted. Use this mode only on a trusted home network, or put HTTPS in front of it (Caddy or Cloudflare Tunnel).') }}
          </div>
        </div>
      </template>
      <div v-else class="note mt12">
        <Icon name="info" size="sm" />
        <div v-if="lang === 'en'">
          From a phone or laptop, go through a tunnel: <b>SSH</b> (<code>ssh -L {{ portNum ?? form.port }}:127.0.0.1:{{ portNum ?? form.port }} root@{{ server }}</code>, then open
          <span class="mono">http://127.0.0.1:{{ portNum ?? form.port }}</span>) or <b>Cloudflare Tunnel + Access</b>.
        </div>
        <div v-else>
          Dari HP atau laptop, buka lewat tunnel: <b>SSH</b> (<code>ssh -L {{ portNum ?? form.port }}:127.0.0.1:{{ portNum ?? form.port }} root@{{ server }}</code>, lalu buka
          <span class="mono">http://127.0.0.1:{{ portNum ?? form.port }}</span>) atau <b>Cloudflare Tunnel + Access</b>.
        </div>
      </div>
    </div>

    <SectionTitle :title="L('Cara masuk', 'Sign-in')" />
    <div class="card rows">
      <div class="row">
        <span class="row-ic" :class="{ ok: form.tgLogin }"><Icon name="send" /></span>
        <div class="grow">
          <div class="label">{{ L('Kode lewat Telegram', 'Code on Telegram') }} <EffectBadge type="live" /></div>
          <div class="hint">{{ L('Kode 6 digit dikirim ke chat pribadi owner, berlaku 5 menit.', "A 6-digit code is sent to the owner's private chat and lasts 5 minutes.") }}</div>
          <div v-if="tgDisabledHint" class="hint">{{ tgDisabledHint }}</div>
          <EnvLock :src="locks.XIAO_WEB_TELEGRAM_LOGIN" />
        </div>
        <Toggle
          v-model="form.tgLogin"
          :label="L('Kode lewat Telegram', 'Code on Telegram')"
          :disabled="!!tgDisabledHint || !!locks.XIAO_WEB_TELEGRAM_LOGIN || (!data.telegram_available && !form.tgLogin)"
        />
      </div>
      <div class="row wrap">
        <span class="row-ic"><Icon name="key" /></span>
        <div class="grow">
          <div class="label">{{ L('Kata sandi cadangan', 'Backup password') }} <EffectBadge type="secret" /></div>
          <div class="hint">{{ L('Untuk saat Telegram tidak bisa dipakai. Disimpan sebagai hash di vault.', 'For when Telegram is unavailable. Stored as a hash in the vault.') }}</div>
        </div>
        <div class="full">
          <SecretField
            secret-key="XIAO_WEB_PASSWORD"
            :meta="data.password"
            :locked="locks.XIAO_WEB_PASSWORD"
            :where-text="L('vault/web, disimpan sebagai hash', 'vault/web, stored as a hash')"
            :delete-warn="L('Masuk hanya bisa lewat kode Telegram.', 'Sign-in then only works with a Telegram code.')"
            :delete-fn="deletePassword"
            external
            @edit="openPassword"
            @changed="reload(true)"
          />
        </div>
      </div>
      <div class="row">
        <span class="row-ic"><Icon name="clock" /></span>
        <div class="grow">
          <div class="label">{{ L('Lama sesi', 'Session length') }}</div>
          <div class="hint">{{ L('Setelah itu harus masuk lagi. Berlaku untuk masuk berikutnya.', 'After this, you sign in again. Applies to new sign-ins.') }}</div>
          <EnvLock :src="locks.XIAO_WEB_SESSION_DAYS" />
        </div>
        <select v-model="form.days" class="select fit" :aria-label="L('Lama sesi', 'Session length')" :disabled="!!locks.XIAO_WEB_SESSION_DAYS">
          <option v-for="o in dayOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
        </select>
      </div>
      <div class="row">
        <span class="row-ic"><Icon name="shield" /></span>
        <div class="grow">
          <div class="label">{{ L('Batas salah masuk', 'Failed sign-in limit') }}</div>
          <div class="hint">{{ L('5 kali salah dalam 15 menit mengunci alamat itu selama 15 menit.', '5 wrong attempts in 15 minutes lock that address for 15 minutes.') }}</div>
        </div>
      </div>
    </div>
    <div class="small muted mt8">
      {{ L('Masuk selalu diperlukan, juga dari 127.0.0.1, karena tunnel meneruskan koneksi sebagai koneksi lokal.', 'Sign-in is always required, also from 127.0.0.1, because a tunnel forwards connections as local ones.') }}
    </div>

    <SectionTitle :title="L('Perangkat yang masuk', 'Signed-in devices')" />
    <div class="card rows">
      <div v-for="s in data.sessions" :key="s.id" class="row">
        <span class="row-ic"><Icon :name="osIcon(s.os)" /></span>
        <div class="grow">
          <div class="label">{{ s.device }} <Badge v-if="s.current" kind="ok">{{ L('Perangkat ini', 'This device') }}</Badge></div>
          <div class="hint">{{ s.ip }}, {{ s.current ? L('baru saja', 'just now') : fmtAgo(s.last_seen) }}</div>
        </div>
        <BusyButton v-if="!s.current" class="btn sm danger" :run="() => revoke(s)" :label="L('Mencabut…', 'Revoking…')">{{ L('Cabut', 'Revoke') }}</BusyButton>
      </div>
      <div v-if="!data.sessions.length" class="empty">{{ L('Tidak ada sesi.', 'No sessions.') }}</div>
    </div>
    <div class="actions mt12">
      <BusyButton class="btn" :run="revokeOthers" :disabled="!others" :label="L('Mencabut…', 'Revoking…')">
        <Icon name="x" size="sm" />{{ L('Cabut perangkat lain', 'Revoke other devices') }}
      </BusyButton>
      <button type="button" class="btn danger" @click="signOutEverywhere"><Icon name="logout" size="sm" />{{ L('Keluar dari semua perangkat', 'Sign out everywhere') }}</button>
    </div>
  </template>

  <Sheet
    :sheet="pwSheet"
    :title="data?.password.set ? L('Ganti kata sandi cadangan', 'Replace the backup password') : L('Pasang kata sandi cadangan', 'Set a backup password')"
    :sub="L('Paling sedikit 8 karakter. Disimpan sebagai hash di vault; sesi yang sudah masuk tetap berlaku.', 'At least 8 characters. Stored as a hash in the vault; signed-in sessions stay valid.')"
  >
    <form @submit.prevent>
      <label class="field">
        <span class="lab">{{ L('Kata sandi baru', 'New password') }}</span>
        <input v-model="pw1" class="input" type="password" autocomplete="new-password" minlength="8" />
      </label>
      <label class="field">
        <span class="lab">{{ L('Ulangi kata sandi', 'Repeat the password') }}</span>
        <input v-model="pw2" class="input" type="password" autocomplete="new-password" minlength="8" />
      </label>
      <div v-if="pwError" class="field-err" role="alert">{{ pwError }}</div>
      <div class="actions end mt16">
        <button type="button" class="btn ghost" @click="pwSheet.hide()">{{ L('Batal', 'Cancel') }}</button>
        <BusyButton type="submit" class="btn primary" :run="savePassword" :label="L('Menyimpan…', 'Saving…')">{{ L('Simpan', 'Save') }}</BusyButton>
      </div>
    </form>
  </Sheet>
</template>
