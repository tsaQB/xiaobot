<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import AppShell from './components/AppShell.vue'
import ConfirmSheet from './components/ConfirmSheet.vue'
import SaveBar from './components/SaveBar.vue'
import Toasts from './components/Toasts.vue'
import { L } from './i18n'
import { restart } from './stores/session'
import { closeSheet, sheetState } from './stores/ui'

const route = useRoute()

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && sheetState.active) {
    e.preventDefault()
    closeSheet()
  }
}

onMounted(() => document.addEventListener('keydown', onKey))
onBeforeUnmount(() => document.removeEventListener('keydown', onKey))
</script>

<template>
  <RouterView v-if="route.name === 'login'" />
  <AppShell v-else-if="route.name" />
  <div class="scrim" :class="{ on: !!sheetState.active }" aria-hidden="true" @click="closeSheet"></div>
  <ConfirmSheet />
  <SaveBar />
  <Toasts />
  <div v-if="restart.running" class="busy-overlay" role="alertdialog" aria-live="assertive" :aria-label="L('Merestart daemon', 'Restarting the daemon')">
    <div class="toast"><span class="spin-inline" aria-hidden="true"></span><span>{{ L('Merestart daemon… Halaman tersambung lagi begitu Xiao hidup.', 'Restarting the daemon… The page reconnects once Xiao is back.') }}</span></div>
  </div>
</template>
