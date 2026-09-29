<script setup lang="ts">
import { L } from '../i18n'
import { confirmRestart } from '../lib/actions'
import { restart } from '../stores/session'
import Icon from './Icon.vue'
</script>

<template>
  <div v-if="restart.needed" class="note warn restart-banner" role="status">
    <Icon name="refresh" size="sm" />
    <div>{{ L('Ada perubahan yang menunggu restart.', 'Some changes wait for a restart.') }}</div>
    <button type="button" class="btn sm" :disabled="restart.running" @click="confirmRestart">
      <Icon name="power" size="sm" />Restart daemon
    </button>
  </div>
</template>
