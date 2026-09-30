<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { L } from '../i18n'
import { toastError } from '../stores/ui'

const props = defineProps<{
  run: () => unknown
  label?: string
  disabled?: boolean
  type?: 'button' | 'submit'
}>()

const busy = ref(false)
let alive = true
onBeforeUnmount(() => {
  alive = false
})

/* Shows a spinner and a label while the action runs; a thrown error becomes a toast. */
async function go(): Promise<void> {
  if (busy.value) return
  busy.value = true
  try {
    await props.run()
  } catch (e) {
    toastError(e)
  } finally {
    if (alive) busy.value = false
  }
}
</script>

<template>
  <button :type="type ?? 'button'" :disabled="disabled || busy" :aria-busy="busy" @click.prevent="go">
    <template v-if="busy"><span class="spin" aria-hidden="true"></span>{{ label ?? L('Memproses…', 'Working…') }}</template>
    <slot v-else />
  </button>
</template>
