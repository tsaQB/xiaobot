import type { CapKey, CapState, RoleId } from '../api/types'
import { L } from '../i18n'

export const ALL_CAPS: readonly CapKey[] = [
  'text_chat',
  'tools',
  'reasoning',
  'image_input',
  'video_input',
  'audio_input',
  'audio_transcription',
  'image_generation',
  'image_editing',
  'structured_output',
  'native_file_input',
]

/** The capabilities worth showing for each role. */
export const ROLE_CAPS: Record<RoleId, CapKey[]> = {
  main: ['text_chat', 'tools', 'reasoning', 'image_input', 'structured_output'],
  vision: ['image_input'],
  video: ['video_input'],
  audio_stt: ['audio_transcription', 'audio_input'],
  image_gen: ['image_generation', 'image_editing'],
  curator: ['text_chat', 'structured_output'],
}

export function capName(k: CapKey): string {
  const names: Record<CapKey, string> = {
    text_chat: L('Chat teks', 'Text chat'),
    tools: 'Tool calling',
    reasoning: 'Reasoning',
    image_input: L('Input gambar', 'Image input'),
    video_input: L('Input video', 'Video input'),
    audio_input: L('Input audio', 'Audio input'),
    audio_transcription: L('Transkripsi', 'Transcription'),
    image_generation: L('Buat gambar', 'Image generation'),
    image_editing: L('Edit gambar', 'Image editing'),
    structured_output: L('JSON terstruktur', 'Structured JSON'),
    native_file_input: L('File native', 'Native files'),
  }
  return names[k]
}

export function capStateText(s: CapState): string {
  return s === 'supported' ? L('ya', 'yes') : s === 'unsupported' ? L('tidak', 'no') : L('belum diuji', 'not tested')
}

export interface RoleMeta {
  name: string
  icon: string
  desc: string
}

export function roleMeta(id: RoleId): RoleMeta {
  const meta: Record<RoleId, RoleMeta> = {
    main: { name: 'Main', icon: 'chat', desc: L('Percakapan utama, alat, dan riwayat.', 'The main conversation, tools and history.') },
    vision: { name: 'Vision', icon: 'eye', desc: L('Membaca gambar dan stiker.', 'Reads pictures and stickers.') },
    video: { name: 'Video', icon: 'video', desc: L('Membaca video dan video note.', 'Reads videos and video notes.') },
    audio_stt: { name: 'Audio STT', icon: 'mic', desc: L('Transkripsi voice note dan audio.', 'Transcribes voice notes and audio.') },
    image_gen: { name: 'Image Generation', icon: 'image', desc: L('Membuat dan mengedit gambar.', 'Creates and edits pictures.') },
    curator: { name: 'Curator', icon: 'filter', desc: L('Merangkum riwayat dan mengekstrak memori.', 'Summarizes history and extracts memories.') },
  }
  return meta[id]
}
