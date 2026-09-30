/*
 * Web chat state. It lives outside the page component, so an answer keeps
 * streaming (and stays on screen) while the owner visits other pages.
 * Web sessions are the CLI sessions (`xiao chat`): same list, same history.
 */
import { reactive } from 'vue'
import { api, errorMessage, seg, streamChat, uploadFile, UPLOAD_LIMIT, type StreamEnd } from '../api/client'
import type { Activity, ChatMessagesState, ChatMessageView, ChatSessionsState, ChatSessionView, FileRef, Ok, UploadRoute } from '../api/types'
import { L } from '../i18n'
import { toast, toastError } from './ui'

export interface UiFile extends FileRef {
  route: UploadRoute | null
}

export interface UiMessage {
  key: string
  role: 'user' | 'assistant'
  text: string
  files: UiFile[]
  at: string | null
  /** Still streaming (or, for a server-side answer, still running). */
  live: boolean
  status: string | null
  tools: { label: string; activity: Activity }[]
  thinking: string | null
  thinkOpen: boolean
  model: string | null
  secs: number | null
  stopped: boolean
  failed: boolean
  error: string | null
  /** The user text this answer replied to, for "Coba lagi". */
  prompt: string | null
}

export interface PendingUpload {
  key: number
  name: string
  size: number
  mime: string
  route: UploadRoute
  id: string | null
  uploading: boolean
}

interface SessionUi {
  messages: UiMessage[] | null
  loading: boolean
  error: string | null
  streaming: boolean
  remoteBusy: boolean
  stopping: boolean
  draft: string
}

export const chat = reactive({
  sessions: [] as ChatSessionView[],
  model: null as string | null,
  activeId: null as number | null,
  loading: false,
  loaded: false,
  error: null as string | null,
  ui: {} as Record<number, SessionUi>,
  pending: [] as PendingUpload[],
  /** Bumped on every change to the visible messages (drives autoscroll). */
  version: 0,
  /** Bumped when the log should jump to the end regardless of position. */
  jump: 0,
  /** Text for the aria-live region. */
  live: '',
})

const ACTIVE_KEY = 'xiao-chat-session'
let seq = 0
let upSeq = 0
let attached = false
const polls = new Map<number, ReturnType<typeof setInterval>>()
const controllers = new Map<number, AbortController>()

/** Stops reading every open answer stream (sign-out); the server keeps what it saved. */
export function abortStreams(): void {
  for (const c of controllers.values()) c.abort()
  controllers.clear()
}

export function sessionUi(id: number): SessionUi {
  let u = chat.ui[id]
  if (!u) {
    chat.ui[id] = { messages: null, loading: false, error: null, streaming: false, remoteBusy: false, stopping: false, draft: '' }
    u = chat.ui[id]
  }
  return u as SessionUi
}

export function routeOf(mime: string | null | undefined): UploadRoute {
  const t = mime ?? ''
  if (t.startsWith('image/')) return 'vision'
  if (t.startsWith('audio/')) return 'stt'
  if (t.startsWith('video/')) return 'video'
  return 'doc'
}

function message(role: UiMessage['role'], fields: Partial<UiMessage> = {}): UiMessage {
  return {
    key: `l${++seq}`,
    role,
    text: '',
    files: [],
    at: null,
    live: false,
    status: null,
    tools: [],
    thinking: null,
    thinkOpen: false,
    model: null,
    secs: null,
    stopped: false,
    failed: false,
    error: null,
    prompt: null,
    ...fields,
  }
}

function fromServer(m: ChatMessageView, i: number): UiMessage {
  return { ...message(m.role, { text: m.text, at: m.at, files: m.files.map((f) => ({ ...f, route: routeOf(f.mime) })) }), key: `s${i}` }
}

function setSessionBusy(id: number, busy: boolean): void {
  const s = chat.sessions.find((x) => x.id === id)
  if (s) s.busy = busy
}

function savedActive(): number | null {
  try {
    const v = Number(localStorage.getItem(ACTIVE_KEY))
    return Number.isInteger(v) && v > 0 ? v : null
  } catch {
    return null
  }
}

/* ---------- Sessions ---------- */

export async function loadSessions(quiet = false, sync = false): Promise<void> {
  if (!quiet) chat.loading = true
  try {
    const st = await api.get<ChatSessionsState>('/api/chat/sessions')
    chat.sessions = st.sessions
    chat.model = st.model
    chat.error = null
    chat.loaded = true
    if (!st.sessions.length) {
      await newSession()
      return
    }
    const has = (id: number | null): id is number => id !== null && st.sessions.some((s) => s.id === id)
    if (!has(chat.activeId)) {
      const pick = [savedActive(), st.active].find(has) ?? st.sessions[0]?.id
      if (pick !== undefined) select(pick)
    } else if (sync) {
      /* Pick up messages written elsewhere (e.g. the terminal) since the last visit. */
      const s = st.sessions.find((x) => x.id === chat.activeId)
      const u = sessionUi(chat.activeId)
      const local = u.messages ? u.messages.filter((m) => m.key !== 'busy').length : 0
      if (s && !u.streaming && u.messages && local < 200 && (local !== s.messages || s.busy)) void loadMessages(s.id, true)
      else if (s && u.remoteBusy) startPoll(s.id)
    }
  } catch (e) {
    if (!chat.loaded) chat.error = errorMessage(e)
  } finally {
    chat.loading = false
  }
}

export function select(id: number): void {
  if (chat.activeId !== id) chat.pending.splice(0)
  chat.activeId = id
  try {
    localStorage.setItem(ACTIVE_KEY, String(id))
  } catch {
    /* storage may be blocked */
  }
  const u = sessionUi(id)
  if (!u.messages && !u.loading) void loadMessages(id)
  else if (u.remoteBusy) startPoll(id)
  chat.jump++
}

export async function newSession(): Promise<void> {
  const s = await api.post<ChatSessionView>('/api/chat/sessions', {})
  chat.sessions.unshift(s)
  sessionUi(s.id).messages = []
  select(s.id)
}

export async function renameSession(id: number, name: string): Promise<void> {
  await api.patch<Ok>(`/api/chat/sessions/${seg(id)}`, { name })
  const s = chat.sessions.find((x) => x.id === id)
  if (s) s.name = name
  toast(L('Nama sesi diganti', 'Session renamed'))
}

export async function clearSession(id: number): Promise<void> {
  await api.post<Ok>(`/api/chat/sessions/${seg(id)}/clear`)
  sessionUi(id).messages = []
  chat.version++
  toast(L('Riwayat sesi dikosongkan', 'Session history cleared'))
  void loadSessions(true)
}

export async function deleteSession(id: number): Promise<void> {
  await api.del<Ok>(`/api/chat/sessions/${seg(id)}`)
  stopPoll(id)
  chat.sessions = chat.sessions.filter((s) => s.id !== id)
  delete chat.ui[id]
  if (chat.activeId === id) chat.activeId = null
  toast(L(`Sesi #${id} dihapus`, `Session #${id} deleted`))
  const first = chat.sessions[0]
  if (first) select(first.id)
  else await newSession()
}

/* ---------- Messages ---------- */

export async function loadMessages(id: number, quiet = false): Promise<void> {
  const u = sessionUi(id)
  if (u.streaming) return
  if (!quiet) u.loading = true
  try {
    const r = await api.get<ChatMessagesState>(`/api/chat/sessions/${seg(id)}/messages`)
    if (u.streaming) return
    const list = r.messages.map(fromServer)
    if (r.busy) {
      list.push({ ...message('assistant', { live: true, status: L('Menjawab…', 'Answering…'), model: chat.model }), key: 'busy' })
    }
    u.messages = list
    u.error = null
    if (u.remoteBusy && !r.busy) u.stopping = false
    u.remoteBusy = r.busy
    setSessionBusy(id, r.busy)
    if (chat.activeId === id) chat.version++
    if (r.busy) startPoll(id)
    else if (polls.has(id)) {
      stopPoll(id)
      void loadSessions(true)
    }
  } catch (e) {
    if (!quiet || !u.messages) u.error = errorMessage(e)
  } finally {
    u.loading = false
  }
}

function startPoll(id: number): void {
  if (polls.has(id) || !attached) return
  polls.set(
    id,
    setInterval(() => {
      if (document.visibilityState === 'visible') void loadMessages(id, true)
    }, 3000),
  )
}

function stopPoll(id: number): void {
  const t = polls.get(id)
  if (t) clearInterval(t)
  polls.delete(id)
}

/** The chat page is on screen: load data and resume polling. */
export function attach(): void {
  attached = true
  void loadSessions(chat.loaded, true)
  if (chat.activeId !== null && sessionUi(chat.activeId).remoteBusy) startPoll(chat.activeId)
}

/** The chat page left the screen: streams continue, polling stops. */
export function detach(): void {
  attached = false
  for (const id of [...polls.keys()]) stopPoll(id)
}

/* ---------- Uploads ---------- */

export function addFiles(files: File[]): void {
  const tooBig = files.filter((f) => f.size > UPLOAD_LIMIT)
  for (const f of files) {
    if (f.size > UPLOAD_LIMIT) continue
    const key = ++upSeq
    chat.pending.push({ key, name: f.name, size: f.size, mime: f.type, route: routeOf(f.type), id: null, uploading: true })
    uploadFile(f)
      .then((v) => {
        const p = chat.pending.find((x) => x.key === key)
        if (!p) return
        p.id = v.id
        p.route = v.route
        p.name = v.name
        p.size = v.size
        p.mime = v.mime
        p.uploading = false
      })
      .catch((e: unknown) => {
        const i = chat.pending.findIndex((x) => x.key === key)
        if (i >= 0) chat.pending.splice(i, 1)
        toast(L(`${f.name} gagal diunggah: ${errorMessage(e)}`, `${f.name} could not be uploaded: ${errorMessage(e)}`), true)
      })
  }
  const big = tooBig[0]
  if (big) toast(L(`${big.name} lebih dari 20 MB dan tidak dilampirkan`, `${big.name} is over 20 MB and was not attached`), true)
}

export function removePending(key: number): void {
  const i = chat.pending.findIndex((p) => p.key === key)
  if (i >= 0) chat.pending.splice(i, 1)
}

/* ---------- Sending, stopping, retrying ---------- */

const QUIET_ACTIVITIES: Activity[] = ['thinking', 'writing']

/** Sends the draft (or `retryText`) with the finished uploads and streams the answer. Returns false when there was nothing to send. */
export async function send(retryText?: string): Promise<boolean> {
  const id = chat.activeId
  if (id === null) return false
  const u = sessionUi(id)
  if (u.streaming || u.remoteBusy) return false
  const retrying = retryText !== undefined
  if (!retrying && chat.pending.some((p) => p.uploading)) {
    toast(L('Tunggu sampai unggahan selesai', 'Wait until the uploads finish'), true)
    return true
  }
  const text = (retrying ? retryText : u.draft).trim()
  const uploads = retrying ? [] : chat.pending.filter((p) => p.id !== null)
  if (!text && !uploads.length) return false

  if (!u.messages) u.messages = []
  u.messages.push(
    message('user', { text, at: new Date().toISOString(), files: uploads.map((p) => ({ id: null, name: p.name, size: p.size, mime: p.mime, route: p.route })) }),
    message('assistant', { live: true, status: L('Berpikir…', 'Thinking…'), prompt: text || null, model: chat.model }),
  )
  const a = u.messages[u.messages.length - 1] as UiMessage
  if (!retrying) {
    u.draft = ''
    chat.pending.splice(0)
  }
  u.streaming = true
  setSessionBusy(id, true)
  chat.live = ''
  chat.version++
  chat.jump++

  const bump = (): void => {
    if (chat.activeId === id) chat.version++
  }
  let end: StreamEnd = 'ended'
  const controller = new AbortController()
  controllers.set(id, controller)
  try {
    end = await streamChat(
      id,
      { text, uploads: uploads.map((p) => p.id).filter((x): x is string => x !== null) },
      {
        status: (e) => {
          if (e.label) a.status = e.label
          if (e.label && !QUIET_ACTIVITIES.includes(e.activity) && a.tools[a.tools.length - 1]?.label !== e.label) {
            a.tools.push({ label: e.label, activity: e.activity })
          }
          bump()
        },
        text: (e) => {
          a.text = e.partial
          a.status = null
          bump()
        },
        done: (e) => {
          a.text = e.answer
          a.thinking = e.thinking
          a.files = e.files.map((f) => ({ ...f, route: routeOf(f.mime) }))
          a.model = e.model
          a.secs = e.secs
          a.stopped = e.stopped
          a.failed = e.failed
          a.live = false
          a.status = null
          a.at = new Date().toISOString()
          chat.live = e.stopped ? L('Jawaban dihentikan.', 'Answer stopped.') : L('Xiao selesai menjawab.', 'Xiao finished answering.')
          bump()
        },
        error: (e) => {
          a.error = e.error || L('Jawaban terputus.', 'The answer broke off.')
          a.live = false
          a.status = null
          chat.live = a.error
          bump()
        },
      },
      controller.signal,
    )
  } catch (e) {
    a.error = errorMessage(e)
    a.live = false
    a.status = null
    end = 'error'
  } finally {
    controllers.delete(id)
    u.streaming = false
    u.stopping = false
    setSessionBusy(id, false)
  }
  if (end === 'ended' || end === 'aborted') {
    a.live = false
    a.status = null
    /* The connection closed before `done`: show what the server saved (and poll if it is still answering). */
    if (end === 'ended') await loadMessages(id, true)
  }
  bump()
  void loadSessions(true)
  return true
}

export async function stop(): Promise<void> {
  const id = chat.activeId
  if (id === null) return
  const u = sessionUi(id)
  if (!(u.streaming || u.remoteBusy) || u.stopping) return
  u.stopping = true
  try {
    await api.post<Ok>(`/api/chat/sessions/${seg(id)}/stop`)
  } catch (e) {
    u.stopping = false
    toastError(e)
    return
  }
  if (!u.streaming) setTimeout(() => void loadMessages(id, true), 800)
}

/** "Coba lagi": drop the failed answer and its question, then ask again (attachments are not resent). */
export function retry(key: string): void {
  const id = chat.activeId
  if (id === null) return
  const u = sessionUi(id)
  const list = u.messages
  if (!list || u.streaming || u.remoteBusy) return
  const i = list.findIndex((m) => m.key === key)
  const failed = list[i]
  if (!failed) return
  const prompt = failed.prompt
  if (!prompt) {
    toast(L('Lampiran tidak bisa dikirim ulang. Lampirkan lagi, lalu kirim.', 'Attachments cannot be resent. Attach them again, then send.'), true)
    return
  }
  const prev = list[i - 1]
  const from = prev && prev.role === 'user' ? i - 1 : i
  list.splice(from, i - from + 1)
  void send(prompt)
}
