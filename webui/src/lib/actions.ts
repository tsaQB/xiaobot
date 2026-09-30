import { api } from '../api/client'
import { L } from '../i18n'
import { router } from '../router'
import { markSignedOut, restartDaemon } from '../stores/session'
import { closeSheet, confirmAction } from '../stores/ui'

/** The restart confirmation used by the home and system pages and the restart bar. */
export function confirmRestart(): void {
  confirmAction({
    title: L('Restart daemon?', 'Restart the daemon?'),
    text: L(
      'Pesan yang sedang dijawab dihentikan lalu dijawab lagi setelah hidup kembali (antrean tidak hilang). systemd menyalakan Xiao lagi dalam ±5 detik.',
      'Answers in progress stop and are answered again once it is back (the queue is kept). systemd starts Xiao again in about 5 seconds.',
    ),
    label: 'Restart',
    busyLabel: L('Merestart…', 'Restarting…'),
    run: restartDaemon,
  })
}

/** Ends this browser's session and shows the sign-in page. */
export async function signOut(): Promise<void> {
  closeSheet()
  try {
    await api.post('/api/auth/logout')
  } catch {
    /* the cookie may already be gone */
  }
  markSignedOut()
  await router.replace({ name: 'login' })
}
