<script setup lang="ts">
import { L } from '../i18n'
import Badge from './Badge.vue'

defineProps<{ type: 'live' | 'restart' | 'secret' | 'gateway' }>()
</script>

<template>
  <Badge v-if="type === 'live'" kind="ok" icon="bolt">{{ L('Langsung berlaku', 'Applies now') }}</Badge>
  <Badge v-else-if="type === 'restart'" kind="warn" icon="refresh">{{ L('Perlu restart', 'Needs restart') }}</Badge>
  <Badge v-else-if="type === 'secret'" kind="violet" icon="lock">{{ L('Disimpan di vault', 'Stored in vault') }}</Badge>
  <Badge v-else kind="info" icon="refresh">{{ L('Gateway dimulai ulang', 'Gateway restarts') }}</Badge>
</template>
