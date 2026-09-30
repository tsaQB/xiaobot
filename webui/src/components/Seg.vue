<script setup lang="ts" generic="T extends string">
const model = defineModel<T>({ required: true })

defineProps<{ options: { value: T; label: string }[]; label?: string }>()
</script>

<template>
  <div class="seg" role="group" :aria-label="label">
    <button
      v-for="o in options"
      :key="o.value"
      type="button"
      :class="{ on: model === o.value }"
      :aria-pressed="model === o.value"
      @click="model = o.value"
    >
      {{ o.label }}
    </button>
  </div>
</template>
