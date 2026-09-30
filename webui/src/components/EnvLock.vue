<script setup lang="ts">
import { lang } from '../i18n'
import Icon from './Icon.vue'

/* A setting whose environment value wins over the saved one cannot be edited here. */
defineProps<{ src?: string | null }>()
</script>

<template>
  <div v-if="src" class="lock">
    <Icon name="lock" size="sm" />
    <div v-if="lang === 'en'">
      <b>Locked by the environment.</b> This value comes from <code>{{ src }}</code>, which always wins over the saved value. Change or
      remove that line, then restart, for the setting here to apply.
    </div>
    <div v-else>
      <b>Dikunci oleh environment.</b> Nilai ini berasal dari <code>{{ src }}</code>, yang selalu menang atas nilai yang disimpan. Ubah atau
      hapus baris itu, lalu restart, agar pengaturan di sini berlaku.
    </div>
  </div>
</template>
