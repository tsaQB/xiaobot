<script setup lang="ts">
import EffectBadge from './EffectBadge.vue'
import EnvLock from './EnvLock.vue'

/* A whole-number setting with its key, help text and environment lock. */
const model = defineModel<string>({ required: true })

defineProps<{
  label: string
  settingKey: string
  help: string
  effect?: 'live' | 'restart'
  locked?: string | null
  min?: number
  max?: number
  invalid?: string | null
}>()
</script>

<template>
  <div class="field mt0">
    <div class="lab">{{ label }} <EffectBadge :type="effect ?? 'live'" /></div>
    <input
      class="input"
      type="number"
      inputmode="numeric"
      step="1"
      :min="min"
      :max="max"
      :value="model"
      :disabled="!!locked"
      :aria-label="label"
      :aria-invalid="!!invalid"
      @input="model = ($event.target as HTMLInputElement).value"
    />
    <div v-if="invalid" class="field-err">{{ invalid }}</div>
    <div class="help"><code>{{ settingKey }}</code> {{ help }}</div>
    <EnvLock :src="locked" />
  </div>
</template>
