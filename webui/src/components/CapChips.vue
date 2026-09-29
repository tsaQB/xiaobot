<script setup lang="ts">
import type { CapKey, CapMap } from '../api/types'
import { L } from '../i18n'
import { capName } from '../lib/caps'
import Badge from './Badge.vue'

defineProps<{ caps: CapMap; keys: CapKey[] }>()
</script>

<template>
  <div class="badges">
    <template v-for="k in keys" :key="k">
      <Badge v-if="caps[k]?.state === 'supported'" kind="ok" icon="check">{{ capName(k) }}</Badge>
      <Badge v-else-if="caps[k]?.state === 'unsupported'" kind="err" icon="x">{{ capName(k) }}</Badge>
      <Badge v-else>{{ capName(k) }}{{ L(' (belum diuji)', ' (not tested)') }}</Badge>
    </template>
  </div>
</template>
