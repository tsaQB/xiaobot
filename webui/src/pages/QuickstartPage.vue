<script setup lang="ts">
import { computed } from 'vue'
import { api } from '../api/client'
import type { Overview } from '../api/types'
import Badge from '../components/Badge.vue'
import Icon from '../components/Icon.vue'
import LoadState from '../components/LoadState.vue'
import PageHead from '../components/PageHead.vue'
import { useLoad } from '../composables/useLoad'
import { L, lang } from '../i18n'
import { COUNTED_STEPS, countedDone, needLabel, nextStep, setupSteps, type StepNeed } from '../lib/setup'
import { applyOverview } from '../stores/session'

const { data, loading, error, reload } = useLoad(() => api.get<Overview>('/api/overview'), { pollMs: 15_000, onData: applyOverview })

const steps = computed(() => (data.value ? setupSteps(data.value) : []))
const done = computed(() => countedDone(steps.value))
const next = computed(() => nextStep(steps.value))
const pct = computed(() => `${Math.round((done.value / COUNTED_STEPS) * 100)}%`)

const needKind = (n: StepNeed): 'warn' | 'info' | '' => (n === 'required' ? 'warn' : n === 'recommended' ? 'info' : '')
</script>

<template>
  <PageHead
    :title="L('Mulai cepat', 'Quickstart')"
    :text="L('Langkah menyiapkan Xiao. Kerjakan yang wajib dulu; sisanya bisa menyusul kapan saja.', 'The steps to set up Xiao. Do the required ones first; the rest can follow any time.')"
  />

  <LoadState v-if="!data" :loading="loading" :error="error" @retry="reload()" />

  <template v-else>
    <div class="card card-body qs-progress">
      <div class="qs-count">
        <template v-if="lang === 'en'"><b>{{ done }}</b> of {{ COUNTED_STEPS }} key steps done</template>
        <template v-else><b>{{ done }}</b> dari {{ COUNTED_STEPS }} langkah penting selesai</template>
      </div>
      <div
        class="meter mt8"
        role="progressbar"
        :aria-valuenow="done"
        aria-valuemin="0"
        :aria-valuemax="COUNTED_STEPS"
        :aria-label="L('Kemajuan penyiapan', 'Setup progress')"
      >
        <i :style="{ width: pct }"></i>
      </div>
      <div v-if="data.setup.complete" class="note ok mt12">
        <Icon name="check" size="sm" />
        <div>{{ L('Yang penting sudah siap: provider, kanal, dan cara masuk. Langkah lain bersifat tambahan.', 'The essentials are ready: a provider, a channel and a way to sign in. The other steps are extras.') }}</div>
      </div>
    </div>

    <ol class="card rows qs-steps mt16">
      <li v-for="(s, i) in steps" :key="s.id" class="row top" :class="{ 'is-done': s.done, 'is-next': next?.id === s.id }">
        <span class="qs-ic" :class="s.done ? 'ok' : ''">
          <Icon :name="s.done ? 'check-circle' : 'circle'" />
          <span class="sr">{{ s.done ? L('Selesai', 'Done') : L('Belum', 'Not done') }}</span>
        </span>
        <div class="grow">
          <div class="label">
<span><span class="qs-n">{{ i + 1 }}.</span> {{ s.title }}</span>
            <Badge v-if="!s.done" :kind="needKind(s.need)">{{ needLabel(s.need) }}</Badge>
          </div>
          <div class="hint">{{ s.why }}</div>
          <div v-if="s.note" class="hint warn-text">{{ s.note }}</div>
        </div>
        <RouterLink class="btn sm" :class="next?.id === s.id ? 'primary' : s.done ? 'ghost' : ''" :to="s.to">
          {{ s.done ? L('Lihat', 'View') : s.action }}<Icon name="chev" size="sm" />
        </RouterLink>
      </li>
    </ol>
  </template>
</template>
