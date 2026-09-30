<script setup lang="ts">
import { fmtBytes, fmtWhen } from '../../format'
import { L, nf } from '../../i18n'
import type { UiMessage } from '../../stores/chat'
import Badge from '../Badge.vue'
import Brandmark from '../Brandmark.vue'
import Icon from '../Icon.vue'
import Markdown from '../Markdown'
import { activityIcon, fileUrl, routeIcon } from './chatFiles'

/* One chat bubble: the owner's text on the right, Xiao's Markdown answer on the left. */
defineProps<{ m: UiMessage; last: boolean; busy: boolean }>()
defineEmits<{ retry: []; copy: [] }>()

function onToggle(m: UiMessage, e: Event): void {
  m.thinkOpen = (e.target as HTMLDetailsElement).open
}
</script>

<template>
  <div v-if="m.role === 'user'" class="msg me">
    <div class="col">
      <div v-if="m.files.length" class="chat-files">
        <span v-for="(f, i) in m.files" :key="i" class="fchip">
          <Icon :name="routeIcon(f.route)" size="sm" />
          <span class="fname" :title="f.name">{{ f.name }}</span>
          <span v-if="f.size !== null" class="fmeta">{{ fmtBytes(f.size) }}</span>
        </span>
      </div>
      <div v-if="m.text" class="bubble"><span class="sr">{{ L('Anda:', 'You:') }} </span>{{ m.text }}</div>
    </div>
  </div>

  <div v-else class="msg">
    <Brandmark class="av" />
    <div class="body">
      <span class="sr">Xiao: </span>
      <details v-if="m.thinking" class="think" :open="m.thinkOpen" @toggle="onToggle(m, $event)">
        <summary><Icon name="chev" size="sm" />{{ L('Proses berpikir', 'Reasoning') }}</summary>
        <div>{{ m.thinking }}</div>
      </details>
      <div v-if="m.tools.length" class="tools">
        <span v-for="(t, i) in m.tools" :key="i" class="tool"><Icon :name="activityIcon(t.activity)" size="sm" /><span>{{ t.label }}</span></span>
      </div>
      <Markdown v-if="m.text && !m.failed" :source="m.text" />
      <div v-if="m.live && m.status" class="msg-status" role="status"><span class="spin-inline" aria-hidden="true"></span>{{ m.status }}</div>
      <div v-if="m.failed || m.error" class="note warn">
        <Icon name="alert" size="sm" />
        <div>
          {{ m.error ?? m.text }}
          <div v-if="last && !busy && m.prompt" class="actions mt8">
            <button type="button" class="btn sm" @click="$emit('retry')"><Icon name="refresh" size="sm" />{{ L('Coba lagi', 'Try again') }}</button>
          </div>
        </div>
      </div>
      <div v-if="m.files.length" class="chat-files mt8">
        <span v-for="(f, i) in m.files" :key="i" class="fchip">
          <Icon :name="routeIcon(f.route)" size="sm" />
          <span class="fname" :title="f.name">{{ f.name }}</span>
          <span v-if="f.size !== null" class="fmeta">{{ fmtBytes(f.size) }}</span>
          <a v-if="f.id" :href="fileUrl(f.id)" download :aria-label="`${L('Unduh', 'Download')} ${f.name}`" :title="L('Unduh', 'Download')">
            <Icon name="down" size="sm" />
          </a>
        </span>
      </div>
      <div v-if="!m.live" class="msg-meta">
        <Badge v-if="m.stopped" kind="warn">{{ L('Dihentikan', 'Stopped') }}</Badge>
        <span v-if="m.secs">{{ nf(m.secs, { maximumFractionDigits: 1 }) }} {{ L('dtk', 's') }}</span>
        <span v-if="m.model && m.secs" class="mono">{{ m.model }}</span>
        <span v-else-if="m.at">{{ fmtWhen(m.at) }}</span>
        <button v-if="m.text && !m.failed" type="button" class="btn sm ghost" @click="$emit('copy')">
          <Icon name="copy" size="sm" />{{ L('Salin', 'Copy') }}
        </button>
      </div>
    </div>
  </div>
</template>
