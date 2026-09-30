<script setup lang="ts">
import { ref } from 'vue'
import { useHeightVar } from '../composables/useHeightVar'
import { L } from '../i18n'
import { confirmRestart } from '../lib/actions'
import { restart } from '../stores/session'
import Icon from './Icon.vue'

/*
 * Floats above the bottom navigation (and above the save bar when both show)
 * while a saved change waits for a restart; the same confirm and restart flow
 * as the buttons on Home and System.
 */
const root = ref<HTMLElement | null>(null)
useHeightVar(root, '--restartbar-h')
</script>

<template>
  <div
    ref="root"
    class="restartbar"
    :class="{ on: restart.needed && !restart.running }"
    role="region"
    :aria-label="L('Menunggu restart', 'Waiting for a restart')"
  >
    <Icon name="refresh" size="sm" />
    <div class="grow">{{ L('Ada perubahan yang menunggu restart.', 'Some changes wait for a restart.') }}</div>
    <button type="button" class="btn primary sm" :disabled="restart.running" @click="confirmRestart">
      <Icon name="power" size="sm" />Restart daemon
    </button>
  </div>
</template>
