<script setup lang="ts">
import { computed, watch } from 'vue'
import { api } from '../api/client'
import type { Effect, SettingRow, SettingsRequest, SystemState, ValueSource, WriteResult } from '../api/types'
import Badge from '../components/Badge.vue'
import EffectBadge from '../components/EffectBadge.vue'
import EnvLock from '../components/EnvLock.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import SectionTitle from '../components/SectionTitle.vue'
import { useForm } from '../composables/useForm'
import { useLoad } from '../composables/useLoad'
import { fmtBytes, fmtDuration, fmtWhen } from '../format'
import { L } from '../i18n'
import { confirmRestart } from '../lib/actions'
import { uptimeNow } from '../stores/session'
import { toast, useSaveBar } from '../stores/ui'

const { data, loading, error, reload } = useLoad(() => api.get<SystemState>('/api/system'))

/* Log level: XIAO_LOG_LEVEL applies at once; RUST_LOG in the environment wins over it. */
type Level = SystemState['system']['log_level']
const LEVELS: readonly Level[] = ['error', 'warn', 'info', 'debug', 'trace']
const { form, dirty, reset } = useForm<{ level: Level }>({ level: 'info' })
const fromState = (d: SystemState): { level: Level } => ({ level: LEVELS.includes(d.system.log_level) ? d.system.log_level : 'info' })
watch(data, (d) => {
  if (d && !dirty.value) reset(fromState(d))
})
const rustLogLock = computed(() => data.value?.env_locks.RUST_LOG ?? null)

function levelHint(l: Level): string {
  return {
    error: L('hanya galat', 'errors only'),
    warn: L('galat dan peringatan', 'errors and warnings'),
    info: L('bawaan', 'default'),
    debug: L('rinci, untuk mencari masalah', 'detailed, for troubleshooting'),
    trace: L('sangat rinci, log cepat penuh', 'very detailed, fills the log fast'),
  }[l]
}

async function save(): Promise<void> {
  if (rustLogLock.value) throw new Error(L('RUST_LOG di environment mengunci level log.', 'RUST_LOG in the environment locks the log level.'))
  const body: SettingsRequest = { XIAO_LOG_LEVEL: form.level }
  await api.put<WriteResult>('/api/settings', body)
  toast(L(`Level log diganti ke ${form.level}. Langsung berlaku.`, `Log level set to ${form.level}. It applies now.`))
  const d = await reload(true)
  if (d) reset(fromState(d))
}

useSaveBar({ dirty, save, discard: () => data.value && reset(fromState(data.value)) })

const c = computed(() => ({
  key: L('Kunci', 'Key'),
  src: L('Asal nilai', 'Source'),
  val: L('Nilai efektif', 'Effective value'),
  eff: L('Perubahan', 'Takes effect'),
}))

function srcName(s: ValueSource): string {
  return { vault: 'vault', environment: 'environment', database: 'database', default: L('bawaan', 'default'), none: L('tidak ada', 'none') }[s]
}

function effect(e: Effect): { text: string; kind: '' | 'ok' | 'warn' | 'err' | 'info' } {
  const t: Record<Effect, { text: string; kind: '' | 'ok' | 'warn' | 'err' | 'info' }> = {
    live: { text: L('langsung', 'now'), kind: 'ok' },
    restart: { text: 'restart', kind: 'warn' },
    gateway: { text: L('gateway ulang', 'gateway restarts'), kind: 'info' },
    locked: { text: L('terkunci', 'locked'), kind: 'err' },
    readonly: { text: L('hanya baca', 'read-only'), kind: '' },
  }
  return t[e]
}

const uptime = computed(() => fmtDuration(uptimeNow.value ?? data.value?.system.uptime_secs ?? 0))
const value = (r: SettingRow): string => r.value || L('(kosong)', '(empty)')

function backup(): void {
  toast(L('Cadangan database sedang diunduh', 'The database backup is downloading'))
  window.location.href = '/api/system/backup'
}
</script>

<template>
  <PageHead
    :title="L('Sistem', 'System')"
    :text="L('Informasi daemon, semua pengaturan beserta asal nilainya, dan tindakan perawatan.', 'Daemon details, every setting with the source of its value, and maintenance actions.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <SectionTitle title="Daemon" />
    <div class="card card-body">
      <dl class="kv">
        <dt>{{ L('Versi', 'Version') }}</dt>
        <dd>xiao {{ data.system.version }}</dd>
        <dt>Host</dt>
        <dd>{{ data.system.host }}</dd>
        <dt>{{ L('Berjalan', 'Running') }}</dt>
        <dd>{{ L(`${uptime}, sejak ${fmtWhen(data.system.started_at)}, PID ${data.system.pid}`, `${uptime}, since ${fmtWhen(data.system.started_at)}, PID ${data.system.pid}`) }}</dd>
        <dt>{{ L('Layanan', 'Service') }}</dt>
        <dd>{{ data.system.service ?? L('tidak di bawah pengelola layanan', 'not under a service manager') }}</dd>
        <dt>Data</dt>
        <dd class="mono">{{ data.system.data_dir }}</dd>
        <dt>{{ L('Konfigurasi', 'Config') }}</dt>
        <dd :class="{ mono: !!data.system.config_file }">{{ data.system.config_file ?? L('tidak ada .env', 'no .env') }}</dd>
        <dt>{{ L('Ukuran', 'Size') }}</dt>
        <dd>
          {{ L(`database ${fmtBytes(data.system.db_bytes)}, lampiran ${fmtBytes(data.system.attachments_bytes)}`, `database ${fmtBytes(data.system.db_bytes)}, attachments ${fmtBytes(data.system.attachments_bytes)}`) }}
        </dd>
      </dl>
      <div v-if="data.restart_keys.length" class="note warn mt16">
        <Icon name="refresh" size="sm" />
        <div>
          {{ L('Menunggu restart:', 'Waiting for a restart:') }}
          <template v-for="(k, i) in data.restart_keys" :key="k"><code>{{ k }}</code><template v-if="i < data.restart_keys.length - 1">, </template></template>
        </div>
      </div>
      <div class="actions mt16">
        <button type="button" class="btn danger" @click="confirmRestart"><Icon name="power" size="sm" />{{ L('Restart daemon', 'Restart daemon') }}</button>
        <button type="button" class="btn" @click="backup"><Icon name="down" size="sm" />{{ L('Unduh cadangan database', 'Download a database backup') }}</button>
      </div>
      <div class="help">
        {{ L('Cadangan berisi database saja (riwayat, memori, pengaturan). Kunci rahasia di vault tidak ikut.', 'The backup holds the database only (history, memory, settings). Secrets in the vault are left out.') }}
      </div>
    </div>

    <SectionTitle :title="L('Log', 'Logs')" />
    <div class="card card-body">
      <label class="field mt0">
        <span class="lab">{{ L('Level log', 'Log level') }} <EffectBadge type="live" /></span>
        <select v-model="form.level" class="select" :disabled="!!rustLogLock" :aria-label="L('Level log', 'Log level')">
          <option v-for="l in LEVELS" :key="l" :value="l">{{ l }} ({{ levelHint(l) }})</option>
        </select>
        <span class="help">
          <code>XIAO_LOG_LEVEL</code>
          {{ L('Filter efektif sekarang:', 'Effective filter now:') }} <code>{{ data.system.rust_log || 'info' }}</code>
        </span>
      </label>
      <EnvLock :src="rustLogLock" />
    </div>

    <SectionTitle :title="L('Semua pengaturan', 'Every setting')" />
    <div class="card">
      <table class="tbl">
        <thead>
          <tr>
            <th>{{ c.key }}</th>
            <th>{{ c.src }}</th>
            <th>{{ c.val }}</th>
            <th>{{ c.eff }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in data.settings" :key="r.key">
            <td :data-l="c.key" class="mono small">{{ r.key }}</td>
            <td :data-l="c.src">
              <Badge v-if="r.source === 'environment'" kind="warn" icon="lock">environment</Badge>
              <Badge v-else-if="r.source === 'vault'" kind="violet" icon="lock">vault</Badge>
              <template v-else>{{ srcName(r.source) }}</template>
            </td>
            <td :data-l="c.val" class="mono small break">{{ value(r) }}</td>
            <td :data-l="c.eff"><Badge :kind="effect(r.effect).kind">{{ effect(r.effect).text }}</Badge></td>
          </tr>
        </tbody>
      </table>
    </div>
    <div class="small muted mt8">
      {{ L('Urutan prioritas: environment (termasuk .env dan Environment= di systemd), lalu database, lalu bawaan. Nilai rahasia tidak pernah dikirim ke browser.', 'Priority order: the environment (including .env and Environment= in systemd), then the database, then the default. Secret values are never sent to the browser.') }}
    </div>
  </template>
</template>
