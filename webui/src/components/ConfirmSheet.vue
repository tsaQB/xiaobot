<script setup lang="ts">
import { L } from '../i18n'
import { confirmState, registerConfirm, useSheet } from '../stores/ui'
import BusyButton from './BusyButton.vue'
import Sheet from './Sheet.vue'

/* The one confirmation dialog of the app (see confirmAction). */
const sheet = useSheet()
registerConfirm(() => sheet.show())

async function go(): Promise<void> {
  const c = confirmState.value
  if (!c) return
  await c.run()
  sheet.hide()
}
</script>

<template>
  <Sheet :sheet="sheet" :title="confirmState?.title" :sub="confirmState?.text">
    <div class="actions end">
      <button type="button" class="btn ghost" @click="sheet.hide()">{{ L('Batal', 'Cancel') }}</button>
      <BusyButton
        class="btn"
        :class="confirmState?.danger === false ? 'primary' : 'danger solid'"
        :run="go"
        :label="confirmState?.busyLabel"
      >
        {{ confirmState?.label }}
      </BusyButton>
    </div>
  </Sheet>
</template>
