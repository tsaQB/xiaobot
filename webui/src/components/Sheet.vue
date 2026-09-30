<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import type { SheetHandle } from '../stores/ui'
import Rich from './Rich'

/*
 * A bottom sheet on phones and a centered dialog on wider screens (same CSS).
 * Focus moves to the first control on open; the global scrim and Escape close it.
 */
const props = defineProps<{ sheet: SheetHandle; title?: string; sub?: string }>()

const el = ref<HTMLElement | null>(null)
const rendered = ref(false)
const titleId = useId()
let hideTimer: ReturnType<typeof setTimeout> | null = null

function focusFirst(): void {
  const first = el.value?.querySelector<HTMLElement>(
    'input:not([disabled]):not([type="hidden"]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), a[href]',
  )
  first?.focus({ preventScroll: true })
}

watch(
  () => props.sheet.open,
  async (open) => {
    if (hideTimer) clearTimeout(hideTimer)
    if (open) {
      rendered.value = true
      await nextTick()
      focusFirst()
    } else {
      /* Keep the content while the sheet slides away. */
      hideTimer = setTimeout(() => {
        if (!props.sheet.open) rendered.value = false
      }, 360)
    }
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  if (hideTimer) clearTimeout(hideTimer)
})
</script>

<template>
  <Teleport to="body">
    <div
      ref="el"
      class="sheet"
      :class="{ on: sheet.open }"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="title ? titleId : undefined"
      :inert="!sheet.open"
    >
      <div class="grab"></div>
      <template v-if="rendered">
        <h3 v-if="title" :id="titleId">{{ title }}</h3>
        <div v-if="sub || $slots.sub" class="sub"><slot name="sub"><Rich :text="sub ?? ''" /></slot></div>
        <slot />
      </template>
    </div>
  </Teleport>
</template>
