<script setup lang="ts">
defineProps<{ name: string; size?: 'sm' | 'lg' }>()
</script>

<template>
  <svg class="i" :class="size" aria-hidden="true" focusable="false"><use :href="`#i-${name}`" /></svg>
</template>
