/** 通用格式化 / 字段读取工具：兼容 snake_case 与 camelCase、分与元两种量纲 */
import { currentLocale, gt } from './i18n'

export type Row = Record<string, unknown>

/** 双语字段（{zh,en} 或纯字符串）→ 当前语言文本 */
export function L(v: unknown): string {
  if (v == null || v === '') return '—'
  if (typeof v === 'string') return v
  if (typeof v === 'number' || typeof v === 'boolean') return String(v)
  if (Array.isArray(v)) return v.map(L).join(', ')
  if (typeof v === 'object') {
    const o = v as Record<string, unknown>
    const en = currentLocale() === 'en-US'
    const pick = en
      ? (o.en ?? o['en-US'] ?? o['en_us'] ?? o.zh ?? o['zh-CN'])
      : (o.zh ?? o['zh-CN'] ?? o['zh_cn'] ?? o.en ?? o['en-US'])
    if (pick != null) return L(pick)
    const first = Object.values(o).find((x) => x != null && typeof x !== 'object')
    return first == null ? '—' : String(first)
  }
  return String(v)
}

/** 按多个候选字段名读取（兼容 snake_case / camelCase） */
export function pick(row: Row | undefined | null, ...keys: string[]): unknown {
  if (!row) return undefined
  for (const k of keys) {
    const v = row[k]
    if (v !== undefined && v !== null && v !== '') return v
  }
  return undefined
}

export function str(v: unknown): string {
  if (v == null || v === '') return '—'
  if (typeof v === 'object') return L(v)
  return String(v)
}

export function num(v: unknown, fallback = 0): number {
  const n = Number(v)
  return Number.isFinite(n) ? n : fallback
}

/** 金额行 → 元：优先读 *_cents（分）÷100，否则读元字段 */
export function toYuan(row: Row, centsKey: string, yuanKey: string): number {
  const camelCents = centsKey.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())
  const c = pick(row, centsKey, camelCents)
  if (c != null) return num(c) / 100
  const y = pick(row, yuanKey)
  if (y != null) return num(y)
  return 0
}

/** 元 → ¥ 格式化 */
export function money(yuan: number): string {
  const n = Number(yuan) || 0
  return (
    gt('common.currency') +
    n.toLocaleString('zh-CN', { minimumFractionDigits: 0, maximumFractionDigits: 2 })
  )
}

/** 大额简写：中文「万」/ 英文 K */
export function wan(yuan: number): string {
  const v = Number(yuan) || 0
  return currentLocale() === 'en-US'
    ? gt('common.currency') + (v / 1000).toFixed(1) + gt('common.kilo')
    : gt('common.currency') + (v / 10000).toFixed(1) + gt('common.wan')
}

/** 提交金额：同时提供契约元字段与 *_cents 分字段（×100），兼容两套后端约定 */
export function centsArgs(key: string, yuan: number): Record<string, number> {
  const v = Math.round(num(yuan))
  return { [key]: v, [`${key}_cents`]: v * 100 }
}

/** 给 camelCase 键补一份 snake_case 别名，后端按任一约定取参都能命中 */
export function aliased(pairs: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...pairs }
  for (const [k, v] of Object.entries(pairs)) {
    const snake = k.replace(/[A-Z]/g, (m) => '_' + m.toLowerCase())
    if (snake !== k && !(snake in out)) out[snake] = v
  }
  return out
}

export function today(): string {
  const d = new Date()
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export function dateAdd(days: number): string {
  const d = new Date()
  d.setDate(d.getDate() + days)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export function nowTime(): string {
  const d = new Date()
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** 状态 → ant-design-vue Tag 颜色 */
export function tagColor(status: string): string {
  const map: Record<string, string> = {
    active: 'green', probation: 'blue', leaving: 'orange', left: 'default',
    onboard: 'blue', offboard: 'red', promote: 'purple', transfer: 'cyan',
    daily: 'blue', business: 'purple', project: 'cyan',
    draft: 'default', pending: 'orange', approved: 'green', rejected: 'red', paid: 'blue',
    running: 'processing', done: 'success', review: 'orange',
    sales: 'blue', purchase: 'purple', labor: 'cyan', service: 'green', nda: 'default',
    effective: 'green', expiring: 'orange', archived: 'default',
    normal: 'green', late: 'orange', early: 'orange', missing: 'red', field: 'blue', leave: 'purple',
    laptop: 'blue', desktop: 'blue', printer: 'purple', usb: 'cyan', disc: 'orange',
    monitor: 'green', projector: 'green', other: 'default',
    instock: 'green', inuse: 'blue', repair: 'orange', scrapped: 'default',
  }
  return map[status] || 'default'
}

export function json(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}
