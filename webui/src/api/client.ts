/*
 * Thin fetch wrapper for the Xiao Console API (see types.ts).
 *
 * The session is an HttpOnly cookie, so every call sends same-origin
 * credentials; every mutation sends `X-Xiao-Request: 1` for the CSRF check.
 */
import { lang } from '../i18n'
import type {
  ApiErrorBody,
  ChatDoneEvent,
  ChatErrorEvent,
  ChatSendRequest,
  ChatStatusEvent,
  ChatTextEvent,
  ErrorCode,
  FileRef,
  UploadView,
} from './types'

export type ClientErrorCode = ErrorCode | 'network'

export class ApiError extends Error {
  readonly code: ClientErrorCode
  readonly status: number
  readonly retryAfter: number | null

  constructor(message: string, code: ClientErrorCode, status: number, retryAfter: number | null = null) {
    super(message)
    this.name = 'ApiError'
    this.code = code
    this.status = status
    this.retryAfter = retryAfter
  }
}

type Json = Record<string, unknown>

function isObject(v: unknown): v is Json {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

const ERROR_CODES: readonly ErrorCode[] = [
  'unauthorized',
  'locked',
  'bad_code',
  'bad_password',
  'not_configured',
  'forbidden',
  'invalid',
  'not_found',
  'conflict',
  'busy',
  'upstream',
  'internal',
]

function isErrorBody(v: unknown): v is ApiErrorBody {
  return isObject(v) && typeof v.error === 'string' && typeof v.code === 'string' && ERROR_CODES.includes(v.code as ErrorCode)
}

/* ---------- Hooks wired up in main.ts ---------- */

let unauthorizedHandler: (() => void) | null = null
let restartObserver: ((needed: boolean) => void) | null = null

/** Called when any call outside /api/auth/* answers `unauthorized`. */
export function onUnauthorized(fn: () => void): void {
  unauthorizedHandler = fn
}

/** Called with `restart_needed` whenever a JSON response carries it. */
export function onRestartFlag(fn: (needed: boolean) => void): void {
  restartObserver = fn
}

function networkError(): ApiError {
  return new ApiError(
    lang.value === 'en' ? 'Cannot reach the daemon. Check that xiao is running.' : 'Daemon tidak bisa dihubungi. Pastikan xiao berjalan.',
    'network',
    0,
  )
}

async function errorFrom(res: Response, path: string): Promise<ApiError> {
  let body: unknown = null
  try {
    body = await res.json()
  } catch {
    /* not JSON, e.g. a proxy error page */
  }
  let err: ApiError
  if (isErrorBody(body)) {
    const msg = lang.value === 'id' && body.error_id ? body.error_id : body.error
    err = new ApiError(msg, body.code, res.status, typeof body.retry_after === 'number' ? body.retry_after : null)
  } else {
    const code: ErrorCode = res.status === 401 ? 'unauthorized' : res.status === 404 ? 'not_found' : res.status >= 500 ? 'internal' : 'invalid'
    err = new ApiError(`HTTP ${res.status}${res.statusText ? ` ${res.statusText}` : ''}`, code, res.status)
  }
  if (err.code === 'unauthorized' && !path.startsWith('/api/auth/') && unauthorizedHandler) unauthorizedHandler()
  return err
}

interface RequestOptions {
  /** Raw body instead of JSON (uploads). */
  raw?: BodyInit
  headers?: Record<string, string>
  signal?: AbortSignal
}

async function send(method: string, path: string, body?: unknown, opts: RequestOptions = {}): Promise<Response> {
  const headers: Record<string, string> = { Accept: 'application/json', ...opts.headers }
  if (method !== 'GET' && method !== 'HEAD') headers['X-Xiao-Request'] = '1'
  let payload: BodyInit | undefined
  if (opts.raw !== undefined) {
    payload = opts.raw
  } else if (body !== undefined) {
    headers['Content-Type'] = 'application/json'
    payload = JSON.stringify(body)
  }
  let res: Response
  try {
    res = await fetch(path, { method, headers, body: payload, credentials: 'same-origin', signal: opts.signal, cache: 'no-store' })
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e
    throw networkError()
  }
  if (!res.ok) throw await errorFrom(res, path)
  return res
}

async function request<T>(method: string, path: string, body?: unknown, opts?: RequestOptions): Promise<T> {
  const res = await send(method, path, body, opts)
  if (res.status === 204) return undefined as T
  const type = res.headers.get('content-type') ?? ''
  if (!type.includes('json')) return undefined as T
  let data: unknown
  try {
    data = await res.json()
  } catch {
    throw new ApiError(lang.value === 'en' ? 'The daemon sent an unreadable answer.' : 'Jawaban daemon tidak bisa dibaca.', 'internal', res.status)
  }
  if (isObject(data) && typeof data.restart_needed === 'boolean' && restartObserver) restartObserver(data.restart_needed)
  return data as T
}

export const api = {
  get: <T>(path: string, signal?: AbortSignal): Promise<T> => request<T>('GET', path, undefined, { signal }),
  post: <T>(path: string, body?: unknown): Promise<T> => request<T>('POST', path, body),
  put: <T>(path: string, body?: unknown): Promise<T> => request<T>('PUT', path, body),
  patch: <T>(path: string, body?: unknown): Promise<T> => request<T>('PATCH', path, body),
  del: <T>(path: string): Promise<T> => request<T>('DELETE', path),
}

/** Path segment helper: ids and keys may contain ':' or '@'. */
export const seg = (v: string | number): string => encodeURIComponent(String(v))

/** Readable message for any thrown value. */
export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) return e.message
  if (e instanceof Error && e.message) return e.message
  return lang.value === 'en' ? 'Something went wrong.' : 'Terjadi kesalahan.'
}

/* ---------- Uploads ---------- */

export const UPLOAD_LIMIT = 20 * 1024 * 1024

export function uploadFile(file: File): Promise<UploadView> {
  return request<UploadView>('POST', '/api/chat/uploads', undefined, {
    raw: file,
    headers: {
      'X-File-Name': encodeURIComponent(file.name),
      'Content-Type': file.type || 'application/octet-stream',
    },
  })
}

/* ---------- Chat stream (POST → text/event-stream) ---------- */

export interface ChatStreamHandlers {
  status: (e: ChatStatusEvent) => void
  text: (e: ChatTextEvent) => void
  done: (e: ChatDoneEvent) => void
  error: (e: ChatErrorEvent) => void
}

/** How a stream ended: a final event arrived, the connection closed early, or the caller aborted. */
export type StreamEnd = 'done' | 'error' | 'ended' | 'aborted'

function str(v: unknown): string | null {
  return typeof v === 'string' ? v : null
}

function toFiles(v: unknown): FileRef[] {
  if (!Array.isArray(v)) return []
  return v.filter(isObject).map((f) => ({
    id: str(f.id),
    name: str(f.name) ?? 'file',
    size: typeof f.size === 'number' ? f.size : null,
    mime: str(f.mime),
  }))
}

export async function streamChat(
  sessionId: number,
  req: ChatSendRequest,
  on: ChatStreamHandlers,
  signal?: AbortSignal,
): Promise<StreamEnd> {
  let res: Response
  try {
    res = await send('POST', `/api/chat/sessions/${seg(sessionId)}/send`, req, {
      headers: { Accept: 'text/event-stream' },
      signal,
    })
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') return 'aborted'
    throw e
  }
  if (!res.body) throw networkError()

  let end: StreamEnd = 'ended'
  const dispatch = (block: string): void => {
    let event = 'message'
    const data: string[] = []
    for (const line of block.split('\n')) {
      if (!line || line.startsWith(':')) continue
      const colon = line.indexOf(':')
      const field = colon < 0 ? line : line.slice(0, colon)
      let value = colon < 0 ? '' : line.slice(colon + 1)
      if (value.startsWith(' ')) value = value.slice(1)
      if (field === 'event') event = value
      else if (field === 'data') data.push(value)
    }
    if (!data.length) return
    let payload: unknown
    try {
      payload = JSON.parse(data.join('\n'))
    } catch {
      return
    }
    if (!isObject(payload)) return
    switch (event) {
      case 'status':
        on.status({ label: str(payload.label) ?? '', activity: (str(payload.activity) ?? 'thinking') as ChatStatusEvent['activity'] })
        break
      case 'text':
        on.text({ partial: str(payload.partial) ?? '' })
        break
      case 'done':
        end = 'done'
        on.done({
          answer: str(payload.answer) ?? '',
          thinking: str(payload.thinking),
          files: toFiles(payload.files),
          model: str(payload.model),
          secs: typeof payload.secs === 'number' ? payload.secs : 0,
          stopped: payload.stopped === true,
          failed: payload.failed === true,
        })
        break
      case 'error':
        end = 'error'
        on.error({ error: str(payload.error) ?? '' })
        break
      default:
        break
    }
  }

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buf += decoder.decode(value, { stream: true })
      buf = buf.replace(/\r\n/g, '\n')
      let idx = buf.indexOf('\n\n')
      while (idx >= 0) {
        dispatch(buf.slice(0, idx))
        buf = buf.slice(idx + 2)
        idx = buf.indexOf('\n\n')
      }
    }
    buf += decoder.decode()
    if (buf.trim()) dispatch(buf)
  } catch (e) {
    if (signal?.aborted || (e instanceof DOMException && e.name === 'AbortError')) return 'aborted'
    if (end === 'ended') return 'ended'
  }
  return end
}
