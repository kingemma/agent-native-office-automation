/**
 * 服务端地址解析：客户端（浏览器 / Windows / macOS / Android / iOS）统一走 REST。
 *
 * 两套地址：
 *   - 局域网（LAN）：办公网内直连，延迟低；
 *   - 外网（WAN）：跨网/远程访问，通常经反向代理。
 *
 * 登录与会话恢复时先探测 LAN 的 `/api/health`（1.2s 超时），可达就用 LAN，
 * 否则自动切 WAN；两个都不通则返回 null，由登录页提示用户检查地址。
 * 配置与上次生效地址都存在 localStorage，刷新/重启后无需重新输入。
 */

export type ApiMode = 'lan' | 'wan'

export interface EndpointConfig {
  lan: string
  wan: string
}

export interface EndpointState extends EndpointConfig {
  mode: ApiMode
  base: string
  latencyMs: number
  checkedAt: string
}

export interface ProbeResult {
  ok: boolean
  ms: number
  error?: string
}

const CFG_KEY = 'oa.api.endpoints'
const ACTIVE_KEY = 'oa.api.active'
export const PROBE_TIMEOUT_MS = 1200

const fallbackBase = import.meta.env.VITE_API_BASE ?? 'http://127.0.0.1:8787'

export const DEFAULT_ENDPOINTS: EndpointConfig = {
  lan: import.meta.env.VITE_API_LAN ?? fallbackBase,
  wan: import.meta.env.VITE_API_WAN ?? fallbackBase,
}

/** 去掉结尾斜杠与空白，避免拼出 `//api/health` */
export function normalizeBase(raw: string): string {
  return (raw || '').trim().replace(/\/+$/, '')
}

export function loadEndpoints(): EndpointConfig {
  if (typeof localStorage === 'undefined') return { ...DEFAULT_ENDPOINTS }
  try {
    const raw = localStorage.getItem(CFG_KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<EndpointConfig>
      return {
        lan: normalizeBase(parsed.lan || DEFAULT_ENDPOINTS.lan),
        wan: normalizeBase(parsed.wan || DEFAULT_ENDPOINTS.wan),
      }
    }
  } catch {
    // 配置损坏时静默回落到默认值，避免登录页白屏
  }
  return { ...DEFAULT_ENDPOINTS }
}

export function saveEndpoints(cfg: EndpointConfig): EndpointConfig {
  const next = { lan: normalizeBase(cfg.lan), wan: normalizeBase(cfg.wan) }
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(CFG_KEY, JSON.stringify(next))
  }
  return next
}

/** 上次生效的地址：页面刷新后先用它发请求，不必等探测 */
function savedActive(): { base: string; mode: ApiMode } {
  if (typeof localStorage !== 'undefined') {
    try {
      const raw = localStorage.getItem(ACTIVE_KEY)
      if (raw) {
        const parsed = JSON.parse(raw) as { base?: string; mode?: ApiMode }
        if (parsed.base) {
          return { base: normalizeBase(parsed.base), mode: parsed.mode === 'wan' ? 'wan' : 'lan' }
        }
      }
    } catch {
      // 同上：坏数据直接忽略
    }
  }
  const cfg = loadEndpoints()
  return { base: cfg.lan || cfg.wan || fallbackBase, mode: 'lan' }
}

const initial = savedActive()
let currentBase = initial.base
let currentMode: ApiMode = initial.mode

export function getBase(): string {
  return currentBase
}

export function getMode(): ApiMode {
  return currentMode
}

/** 由 store 在探测成功或用户手动切换后调用 */
export function applyEndpoint(state: EndpointState): void {
  currentBase = normalizeBase(state.base)
  currentMode = state.mode
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(
      ACTIVE_KEY,
      JSON.stringify({ base: currentBase, mode: currentMode, checkedAt: state.checkedAt }),
    )
  }
}

/** 探测一个地址：`/api/health` 200 且 `ok:true` 视为可达 */
export async function probe(base: string, timeoutMs = PROBE_TIMEOUT_MS): Promise<ProbeResult> {
  const target = normalizeBase(base)
  if (!target) return { ok: false, ms: 0, error: '地址为空' }
  const started = Date.now()
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const res = await fetch(`${target}/api/health`, { signal: controller.signal })
    const ms = Date.now() - started
    if (!res.ok) return { ok: false, ms, error: `HTTP ${res.status}` }
    const body = (await res.json()) as { ok?: boolean }
    return body && body.ok ? { ok: true, ms } : { ok: false, ms, error: '响应异常' }
  } catch (e) {
    return { ok: false, ms: Date.now() - started, error: e instanceof Error ? e.message : String(e) }
  } finally {
    clearTimeout(timer)
  }
}

/**
 * 解析本次应使用的地址：LAN 优先，失败自动切 WAN。
 * 成功后写入 localStorage 并更新当前 base；都失败返回 null（不改当前 base）。
 */
export async function resolveEndpoint(cfg: EndpointConfig = loadEndpoints()): Promise<EndpointState | null> {
  const saved = saveEndpoints(cfg)
  const candidates: { mode: ApiMode; base: string }[] = [{ mode: 'lan', base: saved.lan }]
  if (saved.wan && saved.wan !== saved.lan) candidates.push({ mode: 'wan', base: saved.wan })

  for (const candidate of candidates) {
    if (!candidate.base) continue
    const result = await probe(candidate.base)
    if (result.ok) {
      const state: EndpointState = {
        ...saved,
        mode: candidate.mode,
        base: candidate.base,
        latencyMs: result.ms,
        checkedAt: new Date().toISOString(),
      }
      applyEndpoint(state)
      return state
    }
  }
  return null
}
