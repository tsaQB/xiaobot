<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api, ApiError, errorMessage } from '../api/client'
import type { CodeSent, Ok, SetupRequest } from '../api/types'
import Brandmark from '../components/Brandmark.vue'
import BusyButton from '../components/BusyButton.vue'
import Icon from '../components/Icon.vue'
import Rich from '../components/Rich'
import { now } from '../format'
import { L, lang, toggleLang } from '../i18n'
import { redirectTarget } from '../router'
import { auth, loadAuth, refreshShell } from '../stores/session'

const router = useRouter()
const route = useRoute()

type Step = 'choose' | 'code' | 'password'
const step = ref<Step>('choose')
const digits = ref<string[]>(['', '', '', '', '', ''])
const otp = ref<HTMLInputElement[]>([])
const password = ref('')
const passInput = ref<HTMLInputElement | null>(null)
const expiresAt = ref(0)
const problem = ref<string | null>(null)
const loadingState = ref(false)

const st = computed(() => auth.state)
const tgLogin = computed(() => !!st.value?.telegram_login)
const pwLogin = computed(() => !!st.value?.password_login)
const setupRequired = computed(() => !!st.value?.setup_required)

/* First run: the one-time setup code from the terminal/journal and the first password. */
const setupCode = ref('')
const setupPw1 = ref('')
const setupPw2 = ref('')
const setupCodeInput = ref<HTMLInputElement | null>(null)
const setupPwInput = ref<HTMLInputElement | null>(null)
const setupNote = ref<string | null>(null)

const left = computed(() => Math.max(0, Math.ceil((expiresAt.value - now.value) / 1000)))
const leftText = computed(() => `${Math.floor(left.value / 60)}:${String(left.value % 60).padStart(2, '0')}`)

onMounted(async () => {
  if (!st.value) {
    loadingState.value = true
    await loadAuth()
    loadingState.value = false
  }
  await afterState()
})

async function afterState(): Promise<void> {
  if (setupRequired.value) {
    await nextTick()
    setupCodeInput.value?.focus()
  } else if (!tgLogin.value && pwLogin.value) {
    await showPassword()
  }
}

async function retryState(): Promise<void> {
  loadingState.value = true
  await loadAuth()
  loadingState.value = false
  await afterState()
}

function explain(e: unknown): string {
  if (e instanceof ApiError && e.code === 'locked') {
    const mins = Math.max(1, Math.ceil((e.retryAfter ?? 900) / 60))
    return L(`Terlalu banyak percobaan yang salah. Coba lagi dalam ${mins} menit.`, `Too many wrong attempts. Try again in ${mins} min.`)
  }
  return errorMessage(e)
}

async function sendCode(): Promise<void> {
  problem.value = null
  try {
    const r = await api.post<CodeSent>('/api/auth/code/send')
    expiresAt.value = Date.now() + (r.expires_in || 300) * 1000
    digits.value = ['', '', '', '', '', '']
    step.value = 'code'
    await nextTick()
    otp.value[0]?.focus()
  } catch (e) {
    problem.value = explain(e)
  }
}

async function showPassword(): Promise<void> {
  problem.value = null
  password.value = ''
  step.value = 'password'
  await nextTick()
  passInput.value?.focus()
}

async function finish(): Promise<void> {
  await loadAuth()
  void refreshShell(true)
  await router.replace(redirectTarget(route.query.redirect))
}

async function createFirstPassword(): Promise<void> {
  const code = setupCode.value.trim()
  if (!code) {
    problem.value = L('Masukkan kode setup dari terminal atau journal.', 'Enter the setup code from the terminal or the journal.')
    setupCodeInput.value?.focus()
    return
  }
  if (setupPw1.value.length < 8) {
    problem.value = L('Kata sandi paling sedikit 8 karakter.', 'The password needs at least 8 characters.')
    setupPwInput.value?.focus()
    return
  }
  if (setupPw1.value !== setupPw2.value) {
    problem.value = L('Kedua kata sandi tidak sama.', 'The two passwords do not match.')
    return
  }
  problem.value = null
  const body: SetupRequest = { code, password: setupPw1.value }
  try {
    await api.post<Ok>('/api/auth/setup', body)
    setupPw1.value = ''
    setupPw2.value = ''
    setupCode.value = ''
    await loadAuth()
    void refreshShell(true)
    await router.replace('/quickstart')
  } catch (e) {
    if (e instanceof ApiError && e.code === 'bad_code') {
      problem.value = L(
        'Kode setup salah atau sudah kedaluwarsa. Periksa lagi kode terbaru di terminal atau journal.',
        'The setup code is wrong or has expired. Check the latest code in the terminal or the journal.',
      )
      await nextTick()
      setupCodeInput.value?.select()
    } else if (e instanceof ApiError && e.code === 'conflict') {
      /* Someone (or the CLI) already created a way to sign in: show the normal sign-in instead. */
      setupPw1.value = ''
      setupPw2.value = ''
      await loadAuth()
      setupNote.value = L('Cara masuk sudah dibuat. Silakan masuk.', 'A way to sign in already exists. Please sign in.')
      await afterState()
    } else {
      problem.value = explain(e)
    }
  }
}

async function verify(): Promise<void> {
  const code = digits.value.join('')
  if (code.length !== 6) {
    problem.value = L('Masukkan 6 digit kode dari Telegram.', 'Enter the 6 digits from Telegram.')
    otp.value[digits.value.findIndex((d) => !d)]?.focus()
    return
  }
  problem.value = null
  try {
    await api.post<Ok>('/api/auth/code/verify', { code })
    await finish()
  } catch (e) {
    problem.value = explain(e)
    digits.value = ['', '', '', '', '', '']
    await nextTick()
    otp.value[0]?.focus()
  }
}

async function signInPassword(): Promise<void> {
  if (!password.value) {
    passInput.value?.focus()
    return
  }
  problem.value = null
  try {
    await api.post<Ok>('/api/auth/password', { password: password.value })
    password.value = ''
    await finish()
  } catch (e) {
    problem.value = explain(e)
    password.value = ''
    passInput.value?.focus()
  }
}

/* OTP boxes: digits only, auto-advance, a pasted code fills every box. */
function fill(from: number, text: string): void {
  const chars = text.replace(/\D/g, '').split('')
  if (!chars.length) return
  const next = [...digits.value]
  let i = from
  for (const c of chars) {
    if (i > 5) break
    next[i] = c
    i++
  }
  digits.value = next
  const focusAt = Math.min(i, 5)
  otp.value[focusAt]?.focus()
  if (next.every((d) => d) && chars.length > 1) void verify()
}

function onOtpInput(i: number, e: Event): void {
  const el = e.target as HTMLInputElement
  const raw = el.value.replace(/\D/g, '')
  if (raw.length > 1) {
    fill(i, raw)
    return
  }
  const next = [...digits.value]
  next[i] = raw
  digits.value = next
  el.value = raw
  if (raw && i < 5) otp.value[i + 1]?.focus()
  if (raw && i === 5 && next.every((d) => d)) void verify()
}

function onOtpKey(i: number, e: KeyboardEvent): void {
  if (e.key === 'Backspace' && !digits.value[i] && i > 0) {
    e.preventDefault()
    const next = [...digits.value]
    next[i - 1] = ''
    digits.value = next
    otp.value[i - 1]?.focus()
  } else if (e.key === 'ArrowLeft' && i > 0) {
    otp.value[i - 1]?.focus()
  } else if (e.key === 'ArrowRight' && i < 5) {
    otp.value[i + 1]?.focus()
  }
}

function backToCode(): void {
  problem.value = null
  step.value = 'choose'
}

function onOtpPaste(i: number, e: ClipboardEvent): void {
  const text = e.clipboardData?.getData('text') ?? ''
  if (!text) return
  e.preventDefault()
  fill(i, text)
}

function setOtpRef(el: unknown, i: number): void {
  if (el instanceof HTMLInputElement) otp.value[i] = el
}
</script>

<template>
  <div class="login-wrap">
    <div class="login-card">
      <div class="login-brand">
        <Brandmark />
        <h2>Xiao</h2>
        <div class="muted small">{{ L('Hanya untuk owner', 'Owner only') }}</div>
      </div>

      <div class="card card-body" aria-live="polite">
        <div v-if="loadingState" class="loading sm" role="status">
          <span class="spin-inline" aria-hidden="true"></span>{{ L('Memuat…', 'Loading…') }}
        </div>

        <div v-else-if="!st" class="note err" role="alert">
          <Icon name="alert" size="sm" />
          <div>
            {{ auth.error ?? L('Daemon tidak bisa dihubungi.', 'Cannot reach the daemon.') }}
            <div class="actions">
              <button type="button" class="btn sm" @click="retryState"><Icon name="refresh" size="sm" />{{ L('Coba lagi', 'Retry') }}</button>
            </div>
          </div>
        </div>

        <template v-else>
          <div v-if="problem" class="note err login-note" role="alert"><Icon name="alert" size="sm" /><div>{{ problem }}</div></div>
          <div v-else-if="setupNote && !setupRequired" class="note ok login-note" role="status"><Icon name="check" size="sm" /><div>{{ setupNote }}</div></div>

          <template v-if="setupRequired">
            <h3 class="login-h">{{ L('Buat kata sandi pertama', 'Create the first password') }}</h3>
            <p class="mt0 small muted">
              <Rich
                :text="
                  L(
                    'Kode setup dicetak di terminal tempat `xiao start` berjalan, atau di journal: `journalctl -u xiao | grep -i setup`.',
                    'The setup code is printed in the terminal running `xiao start`, or in the journal: `journalctl -u xiao | grep -i setup`.',
                  )
                "
              />
            </p>
            <form @submit.prevent>
              <label class="field mt0">
                <span class="lab">{{ L('Kode setup', 'Setup code') }}</span>
                <input
                  ref="setupCodeInput"
                  v-model="setupCode"
                  class="input mono"
                  placeholder="1234-5678"
                  inputmode="numeric"
                  autocomplete="one-time-code"
                  autocapitalize="off"
                  spellcheck="false"
                  maxlength="32"
                />
              </label>
              <label class="field">
                <span class="lab">{{ L('Kata sandi baru', 'New password') }}</span>
                <input ref="setupPwInput" v-model="setupPw1" class="input" type="password" autocomplete="new-password" minlength="8" />
                <span class="help">{{ L('Paling sedikit 8 karakter. Dipakai untuk masuk dari browser mana pun.', 'At least 8 characters. Used to sign in from any browser.') }}</span>
              </label>
              <label class="field">
                <span class="lab">{{ L('Ulangi kata sandi', 'Repeat the password') }}</span>
                <input v-model="setupPw2" class="input" type="password" autocomplete="new-password" minlength="8" />
              </label>
              <BusyButton type="submit" class="btn primary block mt16" :run="createFirstPassword" :label="L('Menyimpan…', 'Saving…')">
                <Icon name="key" size="sm" />{{ L('Buat kata sandi dan masuk', 'Create the password and sign in') }}
              </BusyButton>
            </form>
          </template>

          <div v-else-if="!tgLogin && !pwLogin" class="note">
            <Icon name="info" size="sm" />
            <div>
              <Rich
                :text="
                  L(
                    'Belum ada cara masuk. Di server, jalankan `xiao web password` untuk membuat kata sandi, atau atur token bot dan owner Telegram.',
                    'No sign-in method yet. On the server, run `xiao web password` to set a password, or set up the Telegram bot token and owner.',
                  )
                "
              />
            </div>
          </div>

          <template v-else-if="step === 'choose'">
            <p class="mt0">
              <template v-if="lang === 'en'">A sign-in code will be sent to the owner's private chat by <b>{{ st.bot_username ? `@${st.bot_username}` : 'the Telegram bot' }}</b>.</template>
              <template v-else>Kode masuk akan dikirim ke chat pribadi owner oleh <b>{{ st.bot_username ? `@${st.bot_username}` : 'bot Telegram' }}</b>.</template>
            </p>
            <BusyButton class="btn primary block" :run="sendCode" :label="L('Mengirim…', 'Sending…')">
              <Icon name="send" size="sm" />{{ L('Kirim kode ke Telegram', 'Send a code on Telegram') }}
            </BusyButton>
            <button v-if="pwLogin" type="button" class="btn ghost block mt8" @click="showPassword">
              {{ L('Masuk dengan kata sandi', 'Sign in with a password') }}
            </button>
          </template>

          <template v-else-if="step === 'code'">
            <p class="mt0">
              <template v-if="left > 0">
                <template v-if="lang === 'en'">The code is on Telegram. It lasts <b>{{ leftText }}</b>.</template>
                <template v-else>Kode dikirim ke Telegram. Berlaku <b>{{ leftText }}</b>.</template>
              </template>
              <template v-else>{{ L('Kode sudah kedaluwarsa. Kirim ulang untuk kode baru.', 'The code has expired. Send again for a new one.') }}</template>
            </p>
            <form @submit.prevent>
              <div class="otp">
                <input
                  v-for="(d, i) in digits"
                  :key="i"
                  :ref="(el) => setOtpRef(el, i)"
                  class="input"
                  :value="d"
                  inputmode="numeric"
                  pattern="[0-9]*"
                  maxlength="6"
                  :autocomplete="i === 0 ? 'one-time-code' : 'off'"
                  :aria-label="`${L('Digit', 'Digit')} ${i + 1}`"
                  @input="onOtpInput(i, $event)"
                  @keydown="onOtpKey(i, $event)"
                  @paste="onOtpPaste(i, $event)"
                  @focus="($event.target as HTMLInputElement).select()"
                />
              </div>
              <BusyButton type="submit" class="btn primary block mt16" :run="verify" :label="L('Memeriksa…', 'Checking…')">
                {{ L('Masuk', 'Sign in') }}
              </BusyButton>
            </form>
            <BusyButton class="btn ghost block mt8" :run="sendCode" :label="L('Mengirim…', 'Sending…')">{{ L('Kirim ulang', 'Send again') }}</BusyButton>
            <button v-if="pwLogin" type="button" class="btn ghost block mt8" @click="showPassword">
              {{ L('Masuk dengan kata sandi', 'Sign in with a password') }}
            </button>
          </template>

          <template v-else>
            <form @submit.prevent>
              <label class="field mt0">
                <span class="lab">{{ L('Kata sandi', 'Password') }}</span>
                <input ref="passInput" v-model="password" class="input" type="password" autocomplete="current-password" />
              </label>
              <BusyButton type="submit" class="btn primary block mt16" :run="signInPassword" :label="L('Memeriksa…', 'Checking…')">
                {{ L('Masuk', 'Sign in') }}
              </BusyButton>
            </form>
            <button v-if="tgLogin" type="button" class="btn ghost block mt8" @click="backToCode">
              {{ L('Pakai kode Telegram', 'Use a Telegram code') }}
            </button>
          </template>
        </template>
      </div>

      <div class="login-foot">
        <span v-if="st" class="small faint">xiao {{ st.version }}, {{ st.bind }}</span>
        <button type="button" class="btn ghost sm" :lang="lang === 'en' ? 'id' : 'en'" @click="toggleLang">
          {{ lang === 'en' ? 'Bahasa Indonesia' : 'English' }}
        </button>
      </div>
    </div>
  </div>
</template>
