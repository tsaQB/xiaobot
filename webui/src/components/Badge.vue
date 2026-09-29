<script setup lang="ts">
import Icon from './Icon.vue'

defineProps<{ kind?: '' | 'ok' | 'warn' | 'err' | 'info' | 'violet'; icon?: string }>()
</script>

<template>
  <span class="badge" :class="kind"><Icon v-if="icon" :name="icon" /><slot /></span>
</template>
