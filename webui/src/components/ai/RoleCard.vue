<script setup lang="ts">
import { computed } from 'vue'
import { api, seg } from '../../api/client'
import type { ProbeResult, ProviderView, RoleTestResult, RoleView } from '../../api/types'
import { fmtAgo, fmtMs } from '../../format'
import { L } from '../../i18n'
import { isBadOutcome, outcomeText, parseRoute } from '../../lib/ai'
import { ROLE_CAPS, roleMeta } from '../../lib/caps'
import { confirmAction, toast } from '../../stores/ui'
import BusyButton from '../BusyButton.vue'
import CapChips from '../CapChips.vue'
import Icon from '../Icon.vue'

/* One specialist role: its route, what the resolved model can do, and tests. */
const props = defineProps<{ role: RoleView; providers: ProviderView[] }>()
const emit = defineEmits<{ changed: []; override: [] }>()
const route = defineModel<string>({ required: true })

const meta = computed(() => roleMeta(props.role.id))

/* A saved route whose model left the catalogue still needs an option to show. */
const orphan = computed(() => {
  const r = parseRoute(route.value)
  if (r.type !== 'specific') return null
  const p = props.providers.find((x) => x.id === r.provider_id)
  return p && p.models.includes(r.model) ? null : { value: route.value, label: r.model }
})

async function probe(): Promise<void> {
  const r = await api.post<ProbeResult>(`/api/ai/probe/${seg(props.role.id)}`)
  const bad = isBadOutcome(r.outcome)
  toast(L(`${meta.value.name}: pengujian selesai (${outcomeText(r.outcome)})`, `${meta.value.name}: test finished (${outcomeText(r.outcome)})`), bad)
  emit('changed')
}

function testImage(): void {
  confirmAction({
    title: L('Buat gambar uji?', 'Generate a test picture?'),
    text: L('Ini membuat satu gambar dan bisa memakai kredit provider.', 'This creates one picture and may use provider credits.'),
    label: L('Buat gambar', 'Generate'),
    busyLabel: L('Membuat…', 'Generating…'),
    danger: false,
    run: async () => {
      const r = await api.post<RoleTestResult>('/api/ai/test/image_gen')
      if (r.ok) toast(L(`${r.model ?? 'Model'} membuat gambar dalam ${fmtMs(r.ms)}`, `${r.model ?? 'The model'} made a picture in ${fmtMs(r.ms)}`))
      else toast(r.detail || L('Gambar uji gagal dibuat', 'The test picture failed'), true)
      emit('changed')
    },
  })
}
</script>

<template>
  <div class="card">
    <div class="card-head">
      <span class="row-ic"><Icon :name="meta.icon" /></span>
      <div class="grow">
        <h4>{{ meta.name }}</h4>
        <div class="sub">{{ meta.desc }}</div>
      </div>
    </div>
    <div class="card-body">
      <select v-model="route" class="select" :aria-label="`${L('Model untuk', 'Model for')} ${meta.name}`">
        <option value="main_model">{{ L('Ikuti model utama', 'Follow main model') }}</option>
        <option value="disabled">{{ L('Nonaktif', 'Off') }}</option>
        <option v-if="orphan" :value="orphan.value">{{ orphan.label }}</option>
        <optgroup v-for="p in providers" :key="p.id" :label="`${p.name} (${p.endpoint.replace(/^https?:\/\//, '')})`">
          <option v-for="m in p.models" :key="m" :value="`m:${p.id}:${m}`">{{ m }}</option>
        </optgroup>
      </select>
      <div v-if="role.error" class="help err-text">{{ role.error }}</div>
      <div v-else-if="role.model" class="help">
        {{ L('Dipakai', 'Uses') }} <span class="mono">{{ role.model }}</span><template v-if="role.provider"> ({{ role.provider }})</template>
      </div>
      <div v-else class="help">{{ L('Nonaktif', 'Off') }}</div>
      <CapChips class="mt12" :caps="role.caps" :keys="ROLE_CAPS[role.id]" />
      <div class="flex mt12">
        <span class="small faint grow">{{ role.checked_at ? `${L('Diuji', 'Tested')} ${fmtAgo(role.checked_at)}` : L('Belum diuji', 'Not tested yet') }}</span>
        <BusyButton class="btn sm" :run="probe" :label="L('Menguji…', 'Testing…')"><Icon name="play" size="sm" />{{ L('Uji', 'Test') }}</BusyButton>
        <button
          type="button"
          class="btn sm ghost"
          :title="L('Koreksi kemampuan', 'Correct capabilities')"
          :aria-label="L('Koreksi kemampuan', 'Correct capabilities')"
          @click="emit('override')"
        >
          <Icon name="edit" size="sm" />
        </button>
      </div>
      <div v-if="role.id === 'image_gen'" class="actions mt8">
        <button type="button" class="btn sm" @click="testImage"><Icon name="image" size="sm" />{{ L('Buat gambar uji', 'Generate a test picture') }}</button>
      </div>
    </div>
  </div>
</template>
