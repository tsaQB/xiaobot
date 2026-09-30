/*
 * The quickstart checklist, built from Overview.setup. Shared by the
 * quickstart page and the card on Home.
 */
import type { Overview } from '../api/types'
import { L } from '../i18n'

export type StepNeed = 'required' | 'recommended' | 'optional'

export interface SetupStep {
  id: 'provider' | 'telegram' | 'password' | 'whatsapp' | 'search' | 'chat'
  title: string
  why: string
  /** Extra line under `why`, e.g. "set, but not running yet". */
  note: string | null
  done: boolean
  need: StepNeed
  to: string
  action: string
  icon: string
}

/** Steps counted by the progress line (the chat try-out is not a setup step). */
export const COUNTED_STEPS = 5

export function setupSteps(o: Overview): SetupStep[] {
  const s = o.setup
  const tgNeed: StepNeed = s.whatsapp ? 'optional' : 'required'
  return [
    {
      id: 'provider',
      title: L('Provider AI', 'AI provider'),
      why: L('Xiao butuh model AI untuk menjawab. Tambahkan provider, lalu pilih model utama.', 'Xiao needs an AI model to answer. Add a provider, then pick the main model.'),
      note: null,
      done: s.provider,
      need: 'required',
      to: '/ai',
      action: L('Atur provider', 'Set up a provider'),
      icon: 'cpu',
    },
    {
      id: 'telegram',
      title: L('Token bot dan owner Telegram', 'Telegram bot token and owner'),
      why: s.whatsapp
        ? L('Opsional karena WhatsApp sudah tertaut. Token dari @BotFather dan ID akun Telegram Anda.', 'Optional because WhatsApp is linked. A token from @BotFather and your Telegram account id.')
        : L('Kanal utama Xiao. Token dari @BotFather dan ID akun Telegram Anda.', "Xiao's main channel. A token from @BotFather and your Telegram account id."),
      note:
        s.telegram && !s.telegram_running
          ? L('Sudah terisi, tapi belum berjalan. Telegram menyala sendiri beberapa detik kemudian.', 'Set, but not running yet. Telegram starts by itself a few seconds later.')
          : null,
      done: s.telegram,
      need: tgNeed,
      to: '/telegram',
      action: L('Atur Telegram', 'Set up Telegram'),
      icon: 'send',
    },
    {
      id: 'password',
      title: L('Kata sandi cadangan', 'Backup password'),
      why: L('Cara masuk ke Xiao saat kode Telegram tidak bisa dipakai.', 'A way to sign in to Xiao when a Telegram code cannot be used.'),
      note: null,
      done: s.password,
      need: 'recommended',
      to: '/security',
      action: L('Pasang kata sandi', 'Set a password'),
      icon: 'key',
    },
    {
      id: 'whatsapp',
      title: 'WhatsApp',
      why: L('Tautkan nomor agar Xiao juga menjawab di WhatsApp.', 'Link a number so Xiao answers on WhatsApp too.'),
      note: null,
      done: s.whatsapp,
      need: 'optional',
      to: '/whatsapp',
      action: L('Tautkan WhatsApp', 'Link WhatsApp'),
      icon: 'phone',
    },
    {
      id: 'search',
      title: L('Kunci API pencarian', 'Search API key'),
      why: L('Pencarian web lebih stabil dengan kunci Brave, Tavily atau Exa.', 'Web search is steadier with a Brave, Tavily or Exa key.'),
      note: null,
      done: s.search_key,
      need: 'optional',
      to: '/search',
      action: L('Tambah kunci', 'Add a key'),
      icon: 'search',
    },
    {
      id: 'chat',
      title: L('Coba chat', 'Try the chat'),
      why: L('Kirim pesan pertama di sini atau di Telegram untuk memastikan semuanya jalan.', 'Send a first message here or on Telegram to see that everything works.'),
      note: null,
      done: o.storage.messages > 0,
      need: 'optional',
      to: '/chat',
      action: L('Buka chat', 'Open the chat'),
      icon: 'chat',
    },
  ]
}

export function countedDone(steps: SetupStep[]): number {
  return steps.slice(0, COUNTED_STEPS).filter((s) => s.done).length
}

/** The step to do next: the first open required or recommended one, then the first open one. */
export function nextStep(steps: SetupStep[]): SetupStep | null {
  return steps.find((s) => !s.done && s.need !== 'optional') ?? steps.find((s) => !s.done) ?? null
}

export function needLabel(n: StepNeed): string {
  return { required: L('Wajib', 'Required'), recommended: L('Disarankan', 'Recommended'), optional: L('Opsional', 'Optional') }[n]
}
