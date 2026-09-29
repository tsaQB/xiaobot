<script setup lang="ts">
import { useRouter } from 'vue-router'
import { L } from '../i18n'

defineProps<{ on: 'telegram' | 'whatsapp' }>()
const router = useRouter()
</script>

<template>
  <div class="seg chan-switch" role="group" :aria-label="L('Kanal', 'Channel')">
    <button type="button" :class="{ on: on === 'telegram' }" :aria-pressed="on === 'telegram'" @click="router.push('/telegram')">
      Telegram
    </button>
    <button type="button" :class="{ on: on === 'whatsapp' }" :aria-pressed="on === 'whatsapp'" @click="router.push('/whatsapp')">
      WhatsApp
    </button>
  </div>
</template>
