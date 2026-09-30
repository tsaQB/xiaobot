<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ActivityDay } from '../api/types'
import { L, lang, nf } from '../i18n'
import Icon from './Icon.vue'

/*
 * Seven days of owner messages (series 1) and Xiao's answers (series 2) as
 * grouped columns on one axis. Hand-built with CSS: thin bars with rounded
 * data ends, a 2px gap between the pair, hairline gridlines, a legend, a
 * readout on hover, keyboard focus or tap, and the same numbers as a table.
 */
const props = defineProps<{ days: ActivityDay[] }>()

const WD_ID = ['Min', 'Sen', 'Sel', 'Rab', 'Kam', 'Jum', 'Sab']
const WD_EN = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
const WD_ID_LONG = ['Minggu', 'Senin', 'Selasa', 'Rabu', 'Kamis', 'Jumat', 'Sabtu']
const WD_EN_LONG = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']
const MON_ID = ['Jan', 'Feb', 'Mar', 'Apr', 'Mei', 'Jun', 'Jul', 'Agu', 'Sep', 'Okt', 'Nov', 'Des']
const MON_EN = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

interface Col {
  d: ActivityDay
  short: string
  long: string
  day: number
  today: boolean
  label: string
}

/* "YYYY-MM-DD" is a calendar day in the server's zone: read it as a local date, not as UTC midnight. */
function parseDay(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
  if (!m) return null
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]))
}

const cols = computed<Col[]>(() => {
  const en = lang.value === 'en'
  const last = props.days.length - 1
  return props.days.map((d, i) => {
    const dt = parseDay(d.date)
    const wd = dt ? dt.getDay() : 0
    const day = dt ? dt.getDate() : 0
    const mon = dt ? (en ? MON_EN : MON_ID)[dt.getMonth()] ?? '' : ''
    const short = dt ? (en ? WD_EN : WD_ID)[wd] ?? '' : d.date
    const longName = dt ? `${(en ? WD_EN_LONG : WD_ID_LONG)[wd] ?? ''}, ${day} ${mon}` : d.date
    const today = i === last
    const when = today ? `${longName} (${L('hari ini', 'today')})` : longName
    return {
      d,
      short,
      long: longName,
      day,
      today,
      label: L(
        `${when}: ${nf(d.prompts)} pesan owner, ${nf(d.answers)} jawaban`,
        `${when}: ${nf(d.prompts)} owner ${d.prompts === 1 ? 'message' : 'messages'}, ${nf(d.answers)} ${d.answers === 1 ? 'answer' : 'answers'}`,
      ),
    }
  })
})

/* One axis for both series: a clean step (1, 2 or 5 × 10ⁿ) with at most three intervals. */
const scale = computed(() => {
  const max = Math.max(1, ...props.days.flatMap((d) => [d.prompts, d.answers]))
  let step = 1
  for (let mag = 1; ; mag *= 10) {
    const hit = [1, 2, 5].map((m) => m * mag).find((s) => max / s <= 3)
    if (hit) {
      step = hit
      break
    }
  }
  const top = Math.ceil(max / step) * step
  const ticks: number[] = []
  for (let t = 0; t <= top; t += step) ticks.push(t)
  return { top, ticks }
})

const empty = computed(() => props.days.every((d) => d.prompts === 0 && d.answers === 0))
const pct = (v: number): string => `${(v / scale.value.top) * 100}%`

/* Readout: mouse hover, keyboard focus, or a tap that pins it. */
const hover = ref<number | null>(null)
const focused = ref<number | null>(null)
const pinned = ref<number | null>(null)
const activeIdx = computed(() => hover.value ?? focused.value ?? pinned.value)
const activeCol = computed(() => (activeIdx.value === null ? null : cols.value[activeIdx.value] ?? null))

function onEnter(i: number, e: PointerEvent): void {
  if (e.pointerType !== 'touch') hover.value = i
}
function onLeave(e: PointerEvent): void {
  if (e.pointerType !== 'touch') hover.value = null
}
function onFocus(i: number, e: FocusEvent): void {
  const el = e.target
  if (el instanceof HTMLElement && el.matches(':focus-visible')) focused.value = i
}
function onTap(i: number): void {
  pinned.value = pinned.value === i ? null : i
}

const tipSide = computed(() => {
  const i = activeIdx.value
  const n = cols.value.length
  if (i === null) return ''
  if (i === 0) return 'start'
  if (i === n - 1) return 'end'
  return ''
})
const tipLeft = computed(() => {
  const i = activeIdx.value ?? 0
  return `${((i + 0.5) / Math.max(1, cols.value.length)) * 100}%`
})
</script>

<template>
  <div class="act">
    <div class="act-legend" aria-hidden="true">
      <span><i class="sw s1"></i>{{ L('Pesan owner', 'Owner messages') }}</span>
      <span><i class="sw s2"></i>{{ L('Jawaban Xiao', "Xiao's answers") }}</span>
    </div>

    <div class="act-chart" @pointerleave="hover = null">
      <div class="act-y" aria-hidden="true">
        <span v-for="t in scale.ticks" :key="t" :style="{ bottom: pct(t) }">{{ nf(t) }}</span>
      </div>
      <div class="act-area">
        <div class="act-grid" aria-hidden="true">
          <i v-for="t in scale.ticks" :key="t" :style="{ bottom: pct(t) }"></i>
        </div>
        <div class="act-cols" role="group" :aria-label="L('Aktivitas 7 hari terakhir, per hari', 'Activity over the last 7 days, per day')">
          <button
            v-for="(c, i) in cols"
            :key="c.d.date"
            type="button"
            class="act-col"
            :class="{ today: c.today, on: activeIdx === i }"
            :aria-label="c.label"
            @pointerenter="onEnter(i, $event)"
            @pointerleave="onLeave($event)"
            @focus="onFocus(i, $event)"
            @blur="focused = null"
            @click="onTap(i)"
          >
            <span class="act-bars">
              <span class="act-bar s1" :class="{ nz: c.d.prompts > 0 }" :style="{ height: pct(c.d.prompts) }"></span>
              <span class="act-bar s2" :class="{ nz: c.d.answers > 0 }" :style="{ height: pct(c.d.answers) }"></span>
            </span>
          </button>
        </div>
        <div v-if="empty" class="act-empty" aria-hidden="true">{{ L('Belum ada aktivitas minggu ini', 'No activity this week yet') }}</div>
        <div v-if="activeCol" class="act-tip" :class="tipSide" :style="tipSide ? undefined : { left: tipLeft }" aria-hidden="true">
          <div class="act-tip-h">{{ activeCol.long }}<template v-if="activeCol.today"> · {{ L('hari ini', 'today') }}</template></div>
          <div class="act-tip-r"><i class="key s1"></i><b>{{ nf(activeCol.d.prompts) }}</b>{{ L('pesan owner', 'owner messages') }}</div>
          <div class="act-tip-r"><i class="key s2"></i><b>{{ nf(activeCol.d.answers) }}</b>{{ L('jawaban', 'answers') }}</div>
          <div class="act-tip-f">Telegram {{ nf(activeCol.d.telegram) }} · WhatsApp {{ nf(activeCol.d.whatsapp) }}</div>
        </div>
      </div>
      <div class="act-x" aria-hidden="true">
        <span v-for="c in cols" :key="c.d.date" :class="{ today: c.today }"><b>{{ c.short }}</b>{{ c.day || '' }}</span>
      </div>
    </div>

    <details class="raw act-table">
      <summary><Icon name="chev" size="sm" />{{ L('Lihat sebagai tabel', 'View as a table') }}</summary>
      <div class="tbl-scroll">
        <table class="mini-tbl">
        <thead>
          <tr>
            <th scope="col">{{ L('Hari', 'Day') }}</th>
            <th scope="col">{{ L('Pesan owner', 'Owner messages') }}</th>
            <th scope="col">{{ L('Jawaban', 'Answers') }}</th>
            <th scope="col">Telegram</th>
            <th scope="col">WhatsApp</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="c in cols" :key="c.d.date" :class="{ today: c.today }">
            <th scope="row">{{ `${c.short} ${c.day || ''}` }}<template v-if="c.today"> ({{ L('hari ini', 'today') }})</template></th>
            <td>{{ nf(c.d.prompts) }}</td>
            <td>{{ nf(c.d.answers) }}</td>
            <td>{{ nf(c.d.telegram) }}</td>
            <td>{{ nf(c.d.whatsapp) }}</td>
          </tr>
        </tbody>
        </table>
      </div>
    </details>
  </div>
</template>
