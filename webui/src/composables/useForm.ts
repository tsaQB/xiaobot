import { computed, reactive, ref, type ComputedRef } from 'vue'

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T
}

export interface Form<T extends object> {
  form: T
  dirty: ComputedRef<boolean>
  /** Replace the values and make them the new saved baseline. */
  reset: (v: T) => void
  /** The last saved baseline. */
  initial: () => T
  /** Keys whose value differs from the baseline. */
  changed: () => (keyof T)[]
}

/** Editable copy of saved settings with a dirty flag for the save bar. */
export function useForm<T extends object>(init: T): Form<T> {
  const form = reactive(clone(init)) as T
  const base = ref(JSON.stringify(form))
  const dirty = computed(() => JSON.stringify(form) !== base.value)
  const initial = (): T => JSON.parse(base.value) as T
  return {
    form,
    dirty,
    initial,
    reset(v: T) {
      Object.assign(form, clone(v))
      base.value = JSON.stringify(form)
    },
    changed() {
      const was = initial()
      return (Object.keys(form) as (keyof T)[]).filter((k) => JSON.stringify(form[k]) !== JSON.stringify(was[k]))
    },
  }
}

/** A whole number in [min, max] written as text, or null. */
export function intIn(v: string, min: number, max: number): number | null {
  if (!/^\s*-?\d+\s*$/.test(v)) return null
  const n = Number(v)
  return Number.isInteger(n) && n >= min && n <= max ? n : null
}
