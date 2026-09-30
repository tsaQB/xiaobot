<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { useInterval } from '../composables/useLoad'
import { fmtDuration, fmtUntil } from '../format'
import { L, lang, toggleLang } from '../i18n'
import { signOut } from '../lib/actions'
import { bottomItems, moreItems, navGroups, pageTitle, PAGES, type NavPage } from '../nav'
import { auth, refreshShell, shell, uptimeNow } from '../stores/session'
import { useSheet } from '../stores/ui'
import { cycleTheme, theme, themeIcon, themeName } from '../theme'
import Brandmark from './Brandmark.vue'
import Icon from './Icon.vue'
import Sheet from './Sheet.vue'

const route = useRoute()
const page = computed<NavPage>(() => {
  const name = String(route.name ?? 'home')
  return (PAGES as readonly string[]).includes(name) ? (name as NavPage) : 'home'
})

const fc = computed(() => shell.failed)
const channelPages: NavPage[] = ['telegram', 'whatsapp']
const inMore = computed(() => !bottomItems().some((b) => b.id === page.value) && !channelPages.includes(page.value))

function bottomOn(id: NavPage | 'more'): boolean {
  if (id === 'more') return inMore.value
  if (id === 'telegram') return channelPages.includes(page.value)
  return id === page.value
}

const otherLang = computed(() => (lang.value === 'en' ? 'id' : 'en'))
const otherLangName = computed(() => (otherLang.value === 'en' ? 'English' : 'Bahasa Indonesia'))
const botName = computed(() => auth.state?.bot_username ?? shell.botUsername)
const version = computed(() => auth.state?.version ?? shell.version)
const uptime = computed(() => (uptimeNow.value === null ? '…' : fmtDuration(uptimeNow.value)))

const more = useSheet()

onMounted(() => void refreshShell(true))
const poll = useInterval(() => void refreshShell(), 30_000)
poll.start()
</script>

<template>
  <div class="app" :data-page="page">
    <nav class="sidebar" :aria-label="L('Navigasi', 'Navigation')">
      <div class="side-brand">
        <Brandmark />
        <div>
          <b>Xiao</b>
          <small><template v-if="botName">@{{ botName }}</template><span class="ver">v{{ version }}</span></small>
        </div>
      </div>
      <template v-for="g in navGroups()" :key="g.group">
        <div class="side-group">{{ g.group }}</div>
        <RouterLink
          v-for="it in g.items"
          :key="it.id"
          class="side-link"
          :class="{ on: it.id === page }"
          :to="`/${it.id}`"
          :aria-current="it.id === page ? 'page' : undefined"
        >
          <Icon :name="it.icon" /><span>{{ it.title }}</span>
          <span v-if="it.id === 'queue' && fc" class="count" :aria-label="L(`${fc} gagal`, `${fc} failed`)">{{ fc }}</span>
        </RouterLink>
      </template>
      <div class="side-foot">
        <template v-if="auth.state?.owner_id">
          {{ L('Masuk sebagai owner', 'Signed in as owner') }} <b>{{ auth.state.owner_id }}</b><br />
        </template>
        <template v-if="auth.state?.session_expires">{{ L('Sesi berakhir', 'Session ends') }} {{ fmtUntil(auth.state.session_expires) }}</template>
      </div>
    </nav>

    <header class="topbar">
      <Brandmark />
      <h1>{{ pageTitle(page) }}</h1>
      <span class="livepill" :title="L('Daemon berjalan', 'Daemon running')"><span class="dot"></span>{{ uptime }}</span>
      <button type="button" class="iconbtn lang" :lang="otherLang" :title="otherLangName" :aria-label="otherLangName" @click="toggleLang">
        {{ otherLang.toUpperCase() }}
      </button>
      <button
        type="button"
        class="iconbtn"
        :title="`${L('Tema', 'Theme')}: ${themeName(theme)}`"
        :aria-label="L('Ganti tema', 'Change theme')"
        @click="cycleTheme"
      >
        <Icon :name="themeIcon(theme)" />
      </button>
      <button type="button" class="iconbtn only-desk" :title="L('Keluar', 'Sign out')" :aria-label="L('Keluar', 'Sign out')" @click="signOut">
        <Icon name="logout" />
      </button>
    </header>

    <main id="main" class="main">
      <RouterView />
    </main>

    <nav class="bottomnav" :aria-label="L('Navigasi utama', 'Main navigation')">
      <template v-for="b in bottomItems()" :key="b.id">
        <a v-if="b.id === 'more'" href="#" role="button" :class="{ on: bottomOn('more') }" @click.prevent="more.show()">
          <Icon :name="b.icon" /><span>{{ b.title }}</span><span v-if="fc" class="badge-dot">{{ fc }}</span>
        </a>
        <RouterLink v-else :to="`/${b.id}`" :class="{ on: bottomOn(b.id) }" :aria-current="b.id === page ? 'page' : undefined">
          <Icon :name="b.icon" /><span>{{ b.title }}</span>
        </RouterLink>
      </template>
    </nav>

    <Sheet
      :sheet="more"
      :title="L('Menu lainnya', 'More')"
      :sub="L('Bagian Xiao yang tidak ada di bilah bawah.', 'The parts of Xiao that are not in the bottom bar.')"
    >
      <div class="more-grid">
        <RouterLink v-for="it in moreItems()" :key="it.id" :to="`/${it.id}`" @click="more.hide()">
          <span class="more-ic" :class="{ err: it.id === 'queue' && fc }"><Icon :name="it.icon" /></span>
          {{ it.title }}<template v-if="it.id === 'queue' && fc"> ({{ fc }})</template>
        </RouterLink>
      </div>
      <div class="sheet-rows mt16">
        <button type="button" class="row" @click="signOut">
          <span class="row-ic"><Icon name="logout" /></span><span class="grow label">{{ L('Keluar', 'Sign out') }}</span>
        </button>
      </div>
    </Sheet>
  </div>
</template>
