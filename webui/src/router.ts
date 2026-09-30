import { createRouter, createWebHashHistory, type LocationQueryValue, type RouteRecordRaw } from 'vue-router'
import AiPage from './pages/AiPage.vue'
import ChatPage from './pages/ChatPage.vue'
import ContextPage from './pages/ContextPage.vue'
import HomePage from './pages/HomePage.vue'
import LoginPage from './pages/LoginPage.vue'
import LogsPage from './pages/LogsPage.vue'
import McpPage from './pages/McpPage.vue'
import MemoryPage from './pages/MemoryPage.vue'
import QueuePage from './pages/QueuePage.vue'
import QuickstartPage from './pages/QuickstartPage.vue'
import SearchPage from './pages/SearchPage.vue'
import SecurityPage from './pages/SecurityPage.vue'
import SystemPage from './pages/SystemPage.vue'
import TelegramPage from './pages/TelegramPage.vue'
import WhatsAppPage from './pages/WhatsAppPage.vue'
import { auth, loadAuth } from './stores/session'
import { closeSheet } from './stores/ui'

const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/home' },
  { path: '/login', name: 'login', component: LoginPage },
  { path: '/home', name: 'home', component: HomePage },
  { path: '/quickstart', name: 'quickstart', component: QuickstartPage },
  { path: '/chat', name: 'chat', component: ChatPage },
  { path: '/ai', name: 'ai', component: AiPage },
  { path: '/search', name: 'search', component: SearchPage },
  { path: '/mcp', name: 'mcp', component: McpPage },
  { path: '/memory', name: 'memory', component: MemoryPage },
  { path: '/telegram', name: 'telegram', component: TelegramPage },
  { path: '/whatsapp', name: 'whatsapp', component: WhatsAppPage },
  { path: '/queue', name: 'queue', component: QueuePage },
  { path: '/context', name: 'context', component: ContextPage },
  { path: '/logs', name: 'logs', component: LogsPage },
  { path: '/system', name: 'system', component: SystemPage },
  { path: '/security', name: 'security', component: SecurityPage },
  { path: '/:rest(.*)*', redirect: '/home' },
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
  scrollBehavior: () => ({ top: 0 }),
})

/** Where to go after signing in: only in-app paths, never back to the sign-in page. */
export function redirectTarget(q: LocationQueryValue | LocationQueryValue[] | undefined): string {
  const s = typeof q === 'string' ? q : ''
  return s.startsWith('/') && !s.startsWith('//') && !s.startsWith('/login') ? s : '/home'
}

/* Unauthenticated visitors always land on the sign-in page. */
router.beforeEach(async (to) => {
  closeSheet()
  if (!auth.loaded) await loadAuth()
  const signedIn = !!auth.state?.authenticated
  if (to.name === 'login') return signedIn ? redirectTarget(to.query.redirect) : true
  if (!signedIn) return { name: 'login', query: to.fullPath && to.fullPath !== '/home' ? { redirect: to.fullPath } : {} }
  return true
})
