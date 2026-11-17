import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// vite.config.ts 运行在 Node 环境；补充 process 类型声明，避免缺少 @types/node 时 vue-tsc 报错
declare const process: { env: Record<string, string | undefined> }

// Tauri 移动端真机调试时会注入 TAURI_DEV_HOST，需要让 Vite 监听局域网地址
const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 5174 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/crates/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // Windows WebView2 最低 Chrome 105，macOS/iOS Safari 13
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    outDir: 'dist',
  },
})
