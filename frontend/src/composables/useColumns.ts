/**
 * 表格列自定义：按「表 + 用户」记住列的显隐与顺序（localStorage）。
 *
 * 返回一个 reactive 聚合对象，模板里 `model.ordered` / `model.columns` 会自动解包。
 */
import { computed, reactive, ref } from 'vue'

export interface ColumnMeta {
  key: string
  title: string
  /** 不可隐藏的固定列（编号 / 姓名 / 操作列），默认 false */
  locked?: boolean
}

const PREFIX = 'oa.cols.'

export function useColumns<T extends ColumnMeta>(
  tableId: string,
  all: () => T[],
  username: () => string,
) {
  const hidden = ref<string[]>([])
  const order = ref<string[]>([])

  function storageKey(): string {
    return `${PREFIX}${username()}.${tableId}`
  }

  function load(): void {
    try {
      const raw = localStorage.getItem(storageKey())
      if (!raw) return
      const parsed = JSON.parse(raw) as { hidden?: string[]; order?: string[] }
      if (Array.isArray(parsed.hidden)) hidden.value = parsed.hidden
      if (Array.isArray(parsed.order)) order.value = parsed.order
    } catch {
      // 配置损坏时回退到默认列
    }
  }
  function persist(): void {
    try {
      localStorage.setItem(storageKey(), JSON.stringify({ hidden: hidden.value, order: order.value }))
    } catch {
      // 隐私模式 / 存储不可用，静默
    }
  }
  load()

  const byKey = computed(() => new Map(all().map((c) => [c.key, c])))

  /** 全量列按用户顺序（未显式排序的追加到末尾，新增列不丢） */
  const ordered = computed<T[]>(() => {
    const allList = all()
    const seq = order.value.slice()
    for (const c of allList) if (!seq.includes(c.key)) seq.push(c.key)
    return seq.map((k) => byKey.value.get(k)).filter((c): c is T => !!c)
  })

  const lockedKeys = computed(() => new Set(ordered.value.filter((c) => c.locked).map((c) => c.key)))

  /** 实际渲染给 a-table 的列：固定列永远保留，其余按显隐过滤 */
  const columns = computed<T[]>(() =>
    ordered.value.filter((c) => c.locked || !hidden.value.includes(c.key)),
  )

  function isHidden(key: string): boolean {
    return !lockedKeys.value.has(key) && hidden.value.includes(key)
  }

  function toggle(key: string): void {
    if (lockedKeys.value.has(key)) return
    hidden.value = hidden.value.includes(key)
      ? hidden.value.filter((k) => k !== key)
      : [...hidden.value, key]
    persist()
  }

  function move(key: string, dir: -1 | 1): void {
    const seq = ordered.value.map((c) => c.key)
    const idx = seq.indexOf(key)
    const target = idx + dir
    if (idx < 0 || target < 0 || target >= seq.length) return
    const next = seq.slice()
    const [moved] = next.splice(idx, 1)
    next.splice(target, 0, moved)
    order.value = next
    persist()
  }

  function reset(): void {
    hidden.value = []
    order.value = []
    persist()
  }

  return reactive({ ordered, columns, toggle, move, reset, isHidden })
}
