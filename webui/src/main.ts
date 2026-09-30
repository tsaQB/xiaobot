import { createApp } from 'vue'
import { onRestartFlag, onUnauthorized } from './api/client'
import App from './App.vue'
import { applyLang } from './i18n'
import { router } from './router'
import { markSignedOut, restart } from './stores/session'
import './styles.css'
import { applyTheme } from './theme'

applyTheme()
applyLang()

/* Any response that carries `restart_needed` updates the restart banner. */
onRestartFlag((needed) => {
  restart.needed = needed
})

/* An expired session on any call sends the owner to the sign-in page. */
onUnauthorized(() => {
  markSignedOut()
  const current = router.currentRoute.value
  if (current.name !== 'login') {
    void router.replace({ name: 'login', query: current.fullPath !== '/home' ? { redirect: current.fullPath } : {} })
  }
})

createApp(App).use(router).mount('#app')
