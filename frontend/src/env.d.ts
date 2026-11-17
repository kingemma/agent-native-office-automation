/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 兜底地址：LAN / WAN 都没配时使用 */
  readonly VITE_API_BASE?: string
  /** 局域网默认地址（登录页可改） */
  readonly VITE_API_LAN?: string
  /** 外网默认地址（登录页可改） */
  readonly VITE_API_WAN?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}
