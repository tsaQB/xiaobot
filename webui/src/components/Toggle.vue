<script setup lang="ts">
const model = defineModel<boolean>({ required: true })

defineProps<{ disabled?: boolean; label?: string; title?: string }>()
</script>

<template>
  <span class="switch" :title="title">
    <input v-model="model" type="checkbox" role="switch" :disabled="disabled" :aria-label="label" /><span></span>
  </span>
</template>
