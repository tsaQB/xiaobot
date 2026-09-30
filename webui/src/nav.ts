import type { PageId } from './api/types'
import { L } from './i18n'

/* `quickstart` is a UI-only page (it reads /api/overview), so it is not in the API's PageId. */
export type NavPage = Exclude<PageId, 'login'> | 'quickstart'

export interface NavItem {
  id: NavPage
  title: string
  icon: string
}

export interface NavGroup {
  group: string
  items: NavItem[]
}

export const PAGES: readonly NavPage[] = [
  'home',
  'quickstart',
  'chat',
  'ai',
  'search',
  'mcp',
  'memory',
  'telegram',
  'whatsapp',
  'queue',
  'context',
  'logs',
  'system',
  'security',
]

export function navGroups(): NavGroup[] {
  return [
    {
      group: L('Ringkasan', 'Overview'),
      items: [
        { id: 'home', title: L('Beranda', 'Home'), icon: 'home' },
        { id: 'quickstart', title: L('Mulai cepat', 'Quickstart'), icon: 'steps' },
        { id: 'chat', title: 'Chat', icon: 'chat' },
      ],
    },
    {
      group: L('Kecerdasan', 'Intelligence'),
      items: [
        { id: 'ai', title: L('AI dan model', 'AI and models'), icon: 'cpu' },
        { id: 'search', title: L('Pencarian web', 'Web search'), icon: 'search' },
        { id: 'mcp', title: 'MCP', icon: 'plug' },
        { id: 'memory', title: L('Memori', 'Memory'), icon: 'book' },
      ],
    },
    {
      group: L('Kanal', 'Channels'),
      items: [
        { id: 'telegram', title: 'Telegram', icon: 'send' },
        { id: 'whatsapp', title: 'WhatsApp', icon: 'phone' },
      ],
    },
    {
      group: L('Operasional', 'Operations'),
      items: [
        { id: 'queue', title: L('Antrean', 'Queue'), icon: 'inbox' },
        { id: 'context', title: L('Konteks dan sesi', 'Context and sessions'), icon: 'layers' },
        { id: 'logs', title: L('Log', 'Logs'), icon: 'term' },
      ],
    },
    {
      group: L('Sistem', 'System'),
      items: [
        { id: 'system', title: L('Sistem', 'System'), icon: 'sliders' },
        { id: 'security', title: L('Keamanan WebUI', 'WebUI security'), icon: 'shield' },
      ],
    },
  ]
}

export function pageTitle(id: string): string {
  for (const g of navGroups()) for (const it of g.items) if (it.id === id) return it.title
  return 'Xiao'
}

/** Phone bottom bar, Home in the centre; "more" opens the sheet with the rest. */
export function bottomItems(): { id: NavPage | 'more'; title: string; icon: string }[] {
  return [
    { id: 'chat', title: 'Chat', icon: 'chat' },
    { id: 'telegram', title: L('Kanal', 'Channels'), icon: 'send' },
    { id: 'home', title: L('Beranda', 'Home'), icon: 'home' },
    { id: 'ai', title: 'AI', icon: 'cpu' },
    { id: 'more', title: L('Lainnya', 'More'), icon: 'grid' },
  ]
}

export function moreItems(): NavItem[] {
  return [
    { id: 'quickstart', title: L('Mulai cepat', 'Quickstart'), icon: 'steps' },
    { id: 'whatsapp', title: 'WhatsApp', icon: 'phone' },
    { id: 'search', title: L('Pencarian', 'Search'), icon: 'search' },
    { id: 'mcp', title: 'MCP', icon: 'plug' },
    { id: 'memory', title: L('Memori', 'Memory'), icon: 'book' },
    { id: 'queue', title: L('Antrean', 'Queue'), icon: 'inbox' },
    { id: 'context', title: L('Konteks', 'Context'), icon: 'layers' },
    { id: 'logs', title: L('Log', 'Logs'), icon: 'term' },
    { id: 'system', title: L('Sistem', 'System'), icon: 'sliders' },
    { id: 'security', title: L('Keamanan', 'Security'), icon: 'shield' },
  ]
}
