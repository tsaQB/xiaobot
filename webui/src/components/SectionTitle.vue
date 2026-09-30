<script setup lang="ts">
defineProps<{ title: string }>()
</script>

<template>
  <div class="section-title">
    <h3>{{ title }}</h3>
    <span class="spacer"></span>
    <slot />
  </div>
</template>
