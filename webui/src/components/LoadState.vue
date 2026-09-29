<script setup lang="ts">
import { L } from '../i18n'
import Icon from './Icon.vue'

defineProps<{ loading: boolean; error: string | null; small?: boolean }>()
defineEmits<{ retry: [] }>()
</script>

<template>
  <div v-if="error" class="note err" role="alert">
    <Icon name="alert" size="sm" />
    <div>
      <b>{{ L('Gagal memuat data.', 'Could not load the data.') }}</b> {{ error }}
      <div class="actions">
        <button type="button" class="btn sm" @click="$emit('retry')"><Icon name="refresh" size="sm" />{{ L('Coba lagi', 'Retry') }}</button>
      </div>
    </div>
  </div>
  <div v-else-if="loading" class="loading" :class="{ sm: small }" role="status">
    <span class="spin-inline" aria-hidden="true"></span>{{ L('Memuat…', 'Loading…') }}
  </div>
</template>
