/**
 * LLM 回复的 markdown 渲染：marked 解析 + DOMPurify 净化。
 *
 * 为什么必须净化：模型输出属于不可信内容，直接 v-html 等于给回复开了 XSS 通道。
 * 为什么缓存：每次发消息都会整表重渲染历史气泡，同一段文本不值得重复解析。
 * 为什么开 breaks：模型习惯用单个换行分行，严格 CommonMark 会把它们并成一行。
 */
import { Marked } from 'marked'
import DOMPurify from 'dompurify'

const md = new Marked({ gfm: true, breaks: true })

const cache = new Map<string, string>()
const CACHE_LIMIT = 200

let hooked = false

/** 回复里的外链一律新窗口打开：WebView 内直接导航会把整个应用顶掉 */
function ensureLinkHook(): void {
  if (hooked) return
  hooked = true
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    const el = node as Element
    if (el.tagName === 'A' && el.hasAttribute('href')) {
      el.setAttribute('target', '_blank')
      el.setAttribute('rel', 'noopener noreferrer')
    }
  })
}

/** 纯文本（本地意图引擎的回复）也走这里：marked 会正常包成段落并保留换行 */
export function renderMarkdown(src: string): string {
  const text = String(src ?? '')
  if (!text) return ''

  const hit = cache.get(text)
  if (hit !== undefined) return hit

  ensureLinkHook()
  const html = DOMPurify.sanitize(md.parse(text, { async: false }), { ADD_ATTR: ['target'] })

  if (cache.size >= CACHE_LIMIT) cache.clear()
  cache.set(text, html)
  return html
}
