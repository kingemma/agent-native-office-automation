/** vue-i18n 单例：组件内用 useI18n()，非组件代码（store / utils）用 gt() */
import { createI18n } from 'vue-i18n'
import zhCN from './locales/zh-CN'
import enUS from './locales/en-US'

const saved = localStorage.getItem('oa.locale')

export const i18n = createI18n({
  legacy: false,
  locale: saved === 'en-US' ? 'en-US' : 'zh-CN',
  fallbackLocale: 'zh-CN',
  messages: { 'zh-CN': zhCN, 'en-US': enUS },
  missingWarn: false,
  fallbackWarn: false,
})

type GlobalComposer = {
  locale: { value: string }
  t: (key: string) => unknown
}
const g = i18n.global as unknown as GlobalComposer

/** 在非 setup 上下文里翻译（返回字符串） */
export function gt(key: string): string {
  const v = g.t(key)
  return typeof v === 'string' ? v : String(v)
}

export function currentLocale(): string {
  return g.locale.value
}

export function toggleLocale(): void {
  const next = g.locale.value === 'en-US' ? 'zh-CN' : 'en-US'
  g.locale.value = next
  localStorage.setItem('oa.locale', next)
}
