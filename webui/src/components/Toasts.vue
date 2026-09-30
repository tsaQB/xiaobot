<script setup lang="ts">
import { toasts } from '../stores/ui'
import Icon from './Icon.vue'
</script>

<template>
  <div class="toasts" role="status" aria-live="polite">
    <div v-for="t in toasts" :key="t.id" class="toast" :class="{ err: t.err }">
      <Icon :name="t.err ? 'alert' : 'check'" /><span>{{ t.msg }}</span>
    </div>
  </div>
</template>
