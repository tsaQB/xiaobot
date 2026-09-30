import { onBeforeUnmount, onMounted, type Ref } from 'vue'

/**
 * Publishes an element's rendered height as a CSS custom property on <html>
 * (0px while it is hidden), so the floating bars can stack without covering
 * each other even when their text wraps.
 */
export function useHeightVar(el: Ref<HTMLElement | null>, name: string): void {
  let observer: ResizeObserver | null = null
  const sync = (): void => {
    const h = el.value?.offsetHeight ?? 0
    document.documentElement.style.setProperty(name, `${h}px`)
  }
  onMounted(() => {
    sync()
    if (el.value && 'ResizeObserver' in window) {
      observer = new ResizeObserver(sync)
      observer.observe(el.value)
    }
  })
  onBeforeUnmount(() => {
    observer?.disconnect()
    document.documentElement.style.removeProperty(name)
  })
}
