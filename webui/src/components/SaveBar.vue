<script setup lang="ts">
import { L } from '../i18n'
import { saveBar, saveBarOn } from '../stores/ui'
import BusyButton from './BusyButton.vue'

async function save(): Promise<void> {
  if (saveBar.value) await saveBar.value.save()
}
</script>

<template>
  <div class="savebar" :class="{ on: saveBarOn }" role="region" :aria-label="L('Perubahan belum disimpan', 'Unsaved changes')">
    <div class="grow">{{ L('Ada perubahan yang belum disimpan', 'You have unsaved changes') }}</div>
    <button type="button" class="btn ghost sm" @click="saveBar?.discard()">{{ L('Batal', 'Discard') }}</button>
    <BusyButton class="btn primary sm" :run="save" :label="L('Menyimpan…', 'Saving…')">{{ L('Simpan', 'Save') }}</BusyButton>
  </div>
</template>
