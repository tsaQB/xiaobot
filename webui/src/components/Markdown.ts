/*
 * Chat answers as Markdown, rendered token by token into VNodes. No HTML
 * string is ever inserted: markdown-it runs with `html: false`, and this
 * renderer only knows a fixed set of tags. Links open in a new tab and must
 * be http(s) or mailto; images must be http(s) or an inline data: picture.
 */
import MarkdownIt, { type Token } from 'markdown-it'
import { defineComponent, h, type VNode } from 'vue'

const md = new MarkdownIt({ html: false, linkify: true, breaks: false })

type Child = VNode | string

interface Frame {
  kind: 'el' | 'pass' | 'table'
  tag: string
  props: Record<string, string>
  children: Child[]
}

const SAFE_LINK = /^(https?:|mailto:)/i
const SAFE_IMG = /^(https?:\/\/|data:image\/(png|jpe?g|gif|webp|avif);base64,)/i
const TAGS = new Set([
  'p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'li', 'blockquote', 'strong', 'em', 's',
  'thead', 'tbody', 'tr', 'th', 'td',
])

function attr(t: Token, name: string): string | null {
  const v = t.attrGet(name)
  return v === null ? null : String(v)
}

function plain(tokens: Token[] | null): string {
  if (!tokens) return ''
  return tokens.map((t) => (t.children ? plain(t.children) : t.content)).join('')
}

function open(t: Token): Frame {
  const pass: Frame = { kind: 'pass', tag: '', props: {}, children: [] }
  if (t.hidden) return pass
  switch (t.type) {
    case 'link_open': {
      const href = attr(t, 'href')
      if (!href || !SAFE_LINK.test(href)) return pass
      const props: Record<string, string> = { href, target: '_blank', rel: 'noopener noreferrer' }
      const title = attr(t, 'title')
      if (title) props.title = title
      return { kind: 'el', tag: 'a', props, children: [] }
    }
    case 'table_open':
      return { kind: 'table', tag: 'table', props: {}, children: [] }
    case 'ordered_list_open': {
      const start = attr(t, 'start')
      return { kind: 'el', tag: 'ol', props: start && /^\d+$/.test(start) ? { start } : {}, children: [] }
    }
    case 'th_open':
    case 'td_open': {
      const style = attr(t, 'style')
      const props: Record<string, string> = {}
      if (style && /^text-align:(left|right|center)$/.test(style)) props.style = style
      return { kind: 'el', tag: t.tag, props, children: [] }
    }
    default:
      return TAGS.has(t.tag) ? { kind: 'el', tag: t.tag, props: {}, children: [] } : pass
  }
}

function close(f: Frame): Child[] {
  if (f.kind === 'pass') return f.children
  if (f.kind === 'table') return [h('div', { class: 'md-tbl' }, [h('table', f.children)])]
  return [h(f.tag, f.props, f.children)]
}

function leaf(t: Token): Child[] {
  switch (t.type) {
    case 'inline':
      return build(t.children ?? [])
    case 'text':
    case 'html_inline':
    case 'html_block':
      return t.content ? [t.content] : []
    case 'code_inline':
      return [h('code', t.content)]
    case 'softbreak':
      return ['\n']
    case 'hardbreak':
      return [h('br')]
    case 'hr':
      return [h('hr')]
    case 'fence':
    case 'code_block': {
      const lang = (t.info || '').trim().split(/\s+/)[0] ?? ''
      const cls = /^[\w+-]{1,32}$/.test(lang) ? { class: `language-${lang}` } : {}
      return [h('pre', [h('code', cls, t.content.replace(/\n$/, ''))])]
    }
    case 'image': {
      const src = attr(t, 'src')
      const alt = plain(t.children) || t.content
      if (!src || !SAFE_IMG.test(src)) return alt ? [alt] : []
      const props: Record<string, string> = { src, alt, loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' }
      const title = attr(t, 'title')
      if (title) props.title = title
      return [h('img', props)]
    }
    default:
      return t.content ? [t.content] : []
  }
}

function build(tokens: Token[]): Child[] {
  const root: Frame = { kind: 'pass', tag: '', props: {}, children: [] }
  const stack: Frame[] = [root]
  const top = (): Frame => stack[stack.length - 1] ?? root
  for (const t of tokens) {
    if (t.nesting === 1) {
      stack.push(open(t))
    } else if (t.nesting === -1) {
      if (stack.length > 1) {
        const f = stack.pop()
        if (f) top().children.push(...close(f))
      }
    } else {
      top().children.push(...leaf(t))
    }
  }
  while (stack.length > 1) {
    const f = stack.pop()
    if (f) top().children.push(...close(f))
  }
  return root.children
}

export function renderMarkdown(source: string): Child[] {
  return build(md.parse(source, {}))
}

export default defineComponent({
  name: 'Markdown',
  props: { source: { type: String, required: true } },
  setup(props) {
    return () => h('div', { class: 'md' }, renderMarkdown(props.source))
  },
})
