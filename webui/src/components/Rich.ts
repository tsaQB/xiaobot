import { defineComponent, h, type VNode } from 'vue'

/*
 * Renders a translated sentence that contains `code` spans and **bold** parts
 * as text nodes and elements, never as HTML. Anything else stays literal text.
 */
export function richNodes(text: string): (VNode | string)[] {
  const out: (VNode | string)[] = []
  const re = /`([^`]+)`|\*\*([^*]+)\*\*/g
  let last = 0
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push(text.slice(last, m.index))
    if (m[1] !== undefined) out.push(h('code', m[1]))
    else if (m[2] !== undefined) out.push(h('b', m[2]))
    last = re.lastIndex
  }
  if (last < text.length) out.push(text.slice(last))
  return out
}

export default defineComponent({
  name: 'Rich',
  props: { text: { type: String, required: true } },
  setup(props) {
    return () => richNodes(props.text)
  },
})
