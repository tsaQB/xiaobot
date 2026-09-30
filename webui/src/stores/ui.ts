import { computed, onBeforeUnmount, reactive, shallowRef, type Ref } from 'vue'
import { errorMessage } from '../api/client'

/* ---------- Toasts (3.2 s) ---------- */

export interface Toast {
  id: number
  msg: string
  err: boolean
}

export const toasts = reactive<Toast[]>([])
let toastSeq = 0

export function toast(msg: string, err = false): void {
  const id = ++toastSeq
  toasts.push({ id, msg, err })
  setTimeout(() => {
    const i = toasts.findIndex((t) => t.id === id)
    if (i >= 0) toasts.splice(i, 1)
  }, 3200)
}

export function toastError(e: unknown): void {
  toast(errorMessage(e), true)
}

/* ---------- Sheets ----------
   One sheet is visible at a time, like the mockup's single #sheet element.
   Opening a sheet while another is open replaces it and keeps the original
   opener, so focus returns to the control that started the flow. */

export const sheetState = reactive<{ active: symbol | null; opener: HTMLElement | null }>({ active: null, opener: null })

export interface SheetHandle {
  readonly open: boolean
  show(): void
  hide(): void
}

export function closeSheet(): void {
  if (!sheetState.active) return
  sheetState.active = null
  const opener = sheetState.opener
  sheetState.opener = null
  if (opener && opener.isConnected) opener.focus({ preventScroll: true })
}

export function useSheet(): SheetHandle {
  const id = Symbol('sheet')
  const handle: SheetHandle = {
    get open() {
      return sheetState.active === id
    },
    show() {
      if (!sheetState.active) {
        const el = document.activeElement
        sheetState.opener = el instanceof HTMLElement ? el : null
      }
      sheetState.active = id
    },
    hide() {
      if (sheetState.active === id) closeSheet()
    },
  }
  onBeforeUnmount(() => {
    if (sheetState.active === id) {
      sheetState.active = null
      sheetState.opener = null
    }
  })
  return handle
}

/* ---------- Confirm sheet ---------- */

export interface ConfirmOptions {
  title: string
  /** Supports `code` and **bold** (see Rich). */
  text: string
  label: string
  danger?: boolean
  /** Runs while the confirm button shows a spinner; the sheet closes when it resolves. */
  run: () => Promise<unknown> | unknown
  busyLabel?: string
}

export const confirmState = shallowRef<ConfirmOptions | null>(null)
let confirmOpener: (() => void) | null = null

/** Registered by the global ConfirmSheet component. */
export function registerConfirm(open: () => void): void {
  confirmOpener = open
}

export function confirmAction(opts: ConfirmOptions): void {
  confirmState.value = opts
  if (confirmOpener) confirmOpener()
}

/* ---------- Unsaved-changes bar ---------- */

export interface SaveBarRegistration {
  dirty: Ref<boolean>
  save: () => Promise<void>
  discard: () => void
}

export const saveBar = shallowRef<SaveBarRegistration | null>(null)
export const saveBarOn = computed(() => !!saveBar.value && saveBar.value.dirty.value)

/** A page with editable settings registers its dirty flag and save/discard handlers. */
export function useSaveBar(reg: SaveBarRegistration): void {
  saveBar.value = reg
  onBeforeUnmount(() => {
    if (saveBar.value === reg) saveBar.value = null
  })
}

/* ---------- Clipboard (also over plain HTTP on a LAN) ---------- */

export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    /* fall through to the legacy path */
  }
  try {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    ta.remove()
    return ok
  } catch {
    return false
  }
}

export const isDesk = (): boolean => window.matchMedia('(min-width: 900px)').matches
export const isCoarse = (): boolean => window.matchMedia('(pointer: coarse)').matches
