<script setup lang="ts">
import { reactive, watch } from 'vue'
import { api, seg } from '../../api/client'
import type { CapKey, CapOverrideRequest, Ok, RoleView } from '../../api/types'
import { L } from '../../i18n'
import { ALL_CAPS, capName, capStateText } from '../../lib/caps'
import { toast, type SheetHandle } from '../../stores/ui'
import BusyButton from '../BusyButton.vue'
import Seg from '../Seg.vue'
import Sheet from '../Sheet.vue'

type Choice = 'auto' | 'yes' | 'no'

const props = defineProps<{ sheet: SheetHandle; role: RoleView | null }>()
const emit = defineEmits<{ saved: [] }>()

const choices = reactive({} as Record<CapKey, Choice>)

watch(
  () => props.sheet.open,
  (open) => {
    if (!open || !props.role) return
    for (const k of ALL_CAPS) choices[k] = props.role.caps[k]?.override ?? 'auto'
  },
)

const options = (): { value: Choice; label: string }[] => [
  { value: 'auto', label: L('Otomatis', 'Auto') },
  { value: 'yes', label: L('Ya', 'Yes') },
  { value: 'no', label: L('Tidak', 'No') },
]

async function save(): Promise<void> {
  if (!props.role) return
  const body: CapOverrideRequest = { overrides: { ...choices } }
  await api.put<Ok>(`/api/ai/caps/${seg(props.role.id)}`, body)
  props.sheet.hide()
  toast(L('Koreksi disimpan dan langsung dipakai pemilihan model.', 'Corrections saved; model selection uses them now.'))
  emit('saved')
}
</script>

<template>
  <Sheet :sheet="sheet" :title="L('Koreksi kemampuan', 'Correct capabilities')">
    <template #sub>
      <code>{{ role?.model ?? '—' }}</code>.
      {{
        L(
          '“Otomatis” memakai hasil uji dan metadata provider. Koreksi manual tidak kedaluwarsa dan menang atas hasil uji.',
          '“Auto” uses the test results and provider metadata. A manual correction does not expire and wins over test results.',
        )
      }}
    </template>
    <div v-if="role" class="rows">
      <div v-for="k in ALL_CAPS" :key="k" class="row flush">
        <div class="grow">
          {{ capName(k) }}
          <div class="hint">{{ L('Hasil uji', 'Test result') }}: {{ capStateText(role.caps[k]?.state ?? 'unknown') }}</div>
        </div>
        <Seg v-model="choices[k]" :options="options()" :label="capName(k)" />
      </div>
    </div>
    <div class="actions end mt12">
      <button type="button" class="btn ghost" @click="sheet.hide()">{{ L('Batal', 'Cancel') }}</button>
      <BusyButton class="btn primary" :run="save" :label="L('Menyimpan…', 'Saving…')">{{ L('Simpan koreksi', 'Save corrections') }}</BusyButton>
    </div>
  </Sheet>
</template>
