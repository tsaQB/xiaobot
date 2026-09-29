import type { Activity, UploadRoute } from '../../api/types'
import { L } from '../../i18n'

export function routeIcon(r: UploadRoute | null): string {
  return r === 'vision' ? 'image' : r === 'stt' ? 'mic' : r === 'video' ? 'video' : 'file'
}

export function routeLabel(r: UploadRoute): string {
  return r === 'vision' ? 'Vision' : r === 'stt' ? 'Audio STT' : r === 'video' ? 'Video' : L('Dokumen', 'Document')
}

export function activityIcon(a: Activity): string {
  const icons: Record<Activity, string> = {
    thinking: 'spark',
    looking: 'eye',
    reading: 'file',
    searching: 'search',
    fetching: 'link',
    writing: 'edit',
    listening: 'mic',
    drawing: 'image',
    watching: 'video',
    summarizing: 'filter',
    quiz: 'check',
  }
  return icons[a] ?? 'spark'
}

export const fileUrl = (id: string): string => `/api/chat/files/${encodeURIComponent(id)}`
