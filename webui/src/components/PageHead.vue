<script setup lang="ts">
defineProps<{ title: string; text?: string }>()
</script>

<template>
  <div class="page-head">
    <h2>{{ title }}</h2>
    <p v-if="$slots.default || text"><slot>{{ text }}</slot></p>
  </div>
</template>
