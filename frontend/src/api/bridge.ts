/**
 * 前端 API 桥接层：**所有客户端统一走 REST**。
 *
 *   - Windows / macOS / Linux / Android / iOS（Tauri 外壳）→ Axum HTTP
 *   - 普通浏览器（服务器部署）                              → Axum HTTP
 *
 * 不再有 `invoke` 本地通道：桌面端与移动端只是 WebView 外壳，业务与数据都在服务端，
 * 换设备登录看到的是同一份数据。服务端地址由 `endpoint.ts` 解析（局域网优先、外网兜底），
 * 登录页可配置；这里只负责拼接 base 与鉴权头。
 */

import { getBase } from './endpoint'

const TOKEN_KEY = 'oa.token'

/* ---------------- 类型契约 ---------------- */

export interface AuthUser {
  employee_id: string
  username: string
  display_name: string
  role: string
  permissions: string[]
}

export interface AuthSession {
  token: string
  user: AuthUser
}

export interface ToolSpec {
  name: string
  description: string
  parameters: Record<string, unknown>
  domain?: string
  allowed?: boolean
  desc?: unknown
  params?: string
}

export interface TraceStep {
  tool: string
  args: Record<string, unknown>
  result: unknown
  ok: boolean
}

export interface PlanStep {
  tool: string
  args: Record<string, unknown>
}

/** 一步思考：stage = intent / plan / tool / guard / route / llm / done */
export interface ThinkStep {
  stage: string
  text: string
}

/** POST /api/agent/chat 返回：回复 + 运行模式 + 工具轨迹 + 待确认的敏感步骤 */
export interface AgentReply {
  reply: string
  mode: string
  trace: TraceStep[]
  plan?: PlanStep[]
  thinking?: ThinkStep[]
  pending?: PlanStep | null
  affected?: string[]
  needs_confirmation?: boolean
}

/** GET / POST /api/agent/stream 的 SSE 帧 */
export type AgentEvent =
  | { type: 'think'; stage: string; text: string }
  | { type: 'delta'; text: string }
  | { type: 'reply'; reply: AgentReply }
  | { type: 'error'; message: string }

/**
 * 一轮历史对话：服务端不保存会话，多轮追问靠在每次请求里带上最近若干轮。
 * 只传文本，工具调用细节不回灌给模型。
 */
export interface ChatTurn {
  role: 'user' | 'agent'
  text: string
}

export interface AgentPlan {
  steps: PlanStep[]
  reply: string
}

/** 工具调用返回对象：ok 表示是否执行成功，敏感工具可能返回 needs_confirmation */
export interface ToolResult {
  ok: boolean
  needs_confirmation?: boolean
  denied?: boolean
  cancelled?: boolean
  msg?: string
  message?: string
  error?: string
  [key: string]: unknown
}

/** 工作台 KPI（金额单位为分，trend 为近 6 个月费用序列） */
export interface Metrics {
  active_employees: number
  pending_claims: number
  expiring_contracts: number
  asset_value_cents: number
  attendance_rate: number
  budget_used_cents: number
  budget_total_cents: number
  trend: number[]
  trend_months?: string[]
  generated_at?: string
  [key: string]: unknown
}

export interface AuditEvent {
  id: string | number
  tool: string
  actor: string
  payload: unknown
  result?: unknown
  created_at: string
  [key: string]: unknown
}

export interface FeatureItem {
  id: string
  domain: string
  text: string
}

/* ---------------- 令牌 ---------------- */

let token = ''
if (typeof localStorage !== 'undefined') {
  token = localStorage.getItem(TOKEN_KEY) || ''
}

export function setToken(next: string): void {
  token = next
  if (typeof localStorage !== 'undefined') {
    if (next) localStorage.setItem(TOKEN_KEY, next)
    else localStorage.removeItem(TOKEN_KEY)
  }
}

export function getToken(): string {
  return token
}

function authHeaders(): Record<string, string> {
  return token ? { Authorization: `Bearer ${token}` } : {}
}

/* ---------------- HTTP ---------------- */

/**
 * 统一请求：401 清令牌并抛 `UNAUTHORIZED`（上层跳登录页），
 * 网络错误原样抛出，登录页据此提示"无法连接服务器"。
 */
async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(`${getBase()}${path}`, init)
  if (res.status === 401) {
    setToken('')
    throw new Error('UNAUTHORIZED')
  }
  if (!res.ok) throw new Error(`${path} ${res.status}: ${await res.text()}`)
  return (await res.json()) as T
}

async function httpGet<T>(path: string): Promise<T> {
  return request<T>(path, { headers: authHeaders() })
}

async function httpPost<T>(path: string, body: unknown): Promise<T> {
  return request<T>(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify(body),
  })
}

async function httpDelete<T>(path: string): Promise<T> {
  return request<T>(path, { method: 'DELETE', headers: authHeaders() })
}

/** 兼容后端直接返回数组或以 { rows / tools / events } 包装的数组 */
function rowsFrom<T>(data: unknown, ...keys: string[]): T[] {
  if (Array.isArray(data)) return data as T[]
  if (data && typeof data === 'object') {
    const obj = data as Record<string, unknown>
    for (const k of keys) {
      if (Array.isArray(obj[k])) return obj[k] as T[]
    }
  }
  return []
}

/* ---------------- 认证 ---------------- */

export async function login(username: string, password: string): Promise<AuthSession> {
  const session = await httpPost<AuthSession>('/api/auth/login', { username, password })
  setToken(session.token)
  return session
}

export async function logout(): Promise<void> {
  try {
    await httpPost('/api/auth/logout', {})
  } finally {
    setToken('')
  }
}

/** 恢复登录态：本地令牌换当前用户；令牌失效由 401 分支清理 */
export async function me(): Promise<AuthUser | null> {
  if (!token) return null
  try {
    return await httpGet<AuthUser>('/api/auth/me')
  } catch {
    setToken('')
    return null
  }
}

export async function changePassword(oldPassword: string, newPassword: string): Promise<{ ok: boolean }> {
  return httpPost<{ ok: boolean }>('/api/auth/change-password', {
    old_password: oldPassword,
    new_password: newPassword,
  })
}

/* ---------------- 运行时配置（仅管理员） ---------------- */

export interface LlmProviderPreset {
  code: string
  label: string
  base_url: string
  model: string
  needs_key: boolean
}

/** 模型配置视图：api_key 永远是掩码，明文只留在后端 */
export interface LlmSettingsView {
  provider: string
  base_url: string
  model: string
  has_key: boolean
  key_hint: string
  source: 'db' | 'env' | 'none'
  mode: 'llm' | 'local'
  providers: LlmProviderPreset[]
}

export interface LlmSettingsInput {
  provider: string
  base_url: string
  model: string
  /** 留空表示沿用已存 Key */
  api_key: string
  clear?: boolean
}

export async function getLlmSettings(): Promise<LlmSettingsView> {
  return httpGet<LlmSettingsView>('/api/settings/llm')
}

export async function saveLlmSettings(input: LlmSettingsInput): Promise<LlmSettingsView> {
  return httpPost<LlmSettingsView>('/api/settings/llm', input)
}

/** 连通性自检的入参：留空字段由服务端回落到已存配置 */
export interface LlmProbeInput {
  base_url: string
  model: string
  api_key: string
}

/** 连通性自检：用表单当前值发一次最小对话 */
export async function testLlmSettings(
  input: LlmProbeInput,
): Promise<{ ok: boolean; reply: string }> {
  return httpPost<{ ok: boolean; reply: string }>('/api/settings/llm/test', input)
}

/** 列出端点上的可用模型（服务端代理 `GET {base_url}/models`），供模型名下拉使用 */
export async function listLlmModels(baseUrl: string, apiKey: string): Promise<string[]> {
  const data = await httpPost<unknown>('/api/settings/llm/models', {
    base_url: baseUrl,
    api_key: apiKey,
  })
  return rowsFrom<string>(data, 'models')
}

/* ---------------- 业务 API ---------------- */

/** 只读台账（表名由后端白名单校验） */
export async function listTable<T = Record<string, unknown>>(name: string): Promise<T[]> {
  const data = await httpGet<unknown>(`/api/tables/${encodeURIComponent(name)}`)
  return rowsFrom<T>(data, 'rows')
}

/** 拉取 Agent 工具清单（含当前角色是否可用），用于能力面板与权限说明 */
export async function listTools(): Promise<ToolSpec[]> {
  const data = await httpGet<unknown>('/api/tools')
  return rowsFrom<ToolSpec>(data, 'tools')
}

/** 自然语言指令 → Agent 回复（本地意图引擎或 Rig LLM，由服务端模式决定） */
export async function agentChat(message: string): Promise<AgentReply> {
  return httpPost<AgentReply>('/api/agent/chat', { message })
}

/**
 * 流式对话：逐条读 SSE 帧并回调。
 *
 * 不用 `EventSource`——它无法携带 `Authorization: Bearer`，也无法带请求体，
 * 而多轮追问需要把历史一并提交；因此用 `fetch` 的 ReadableStream 手工拆帧。
 */
export async function agentStream(
  message: string,
  history: ChatTurn[],
  onEvent: (event: AgentEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const res = await fetch(`${getBase()}/api/agent/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify({ message, history }),
    signal,
  })
  if (res.status === 401) {
    setToken('')
    throw new Error('UNAUTHORIZED')
  }
  if (!res.ok || !res.body) throw new Error(`/api/agent/stream ${res.status}: ${await res.text()}`)

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  for (;;) {
    const { value, done } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    // SSE 帧以空行分隔；本项目只发 data:，忽略 event:/id:/retry:
    let cut = buffer.indexOf('\n\n')
    while (cut >= 0) {
      const frame = buffer.slice(0, cut)
      buffer = buffer.slice(cut + 2)
      const payload = frame
        .split(/\r?\n/)
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).trim())
        .join('')
      if (payload) onEvent(JSON.parse(payload) as AgentEvent)
      cut = buffer.indexOf('\n\n')
    }
  }
}

/** 预览执行计划：只返回将要调用的工具与参数，不写库 */
export async function agentPlan(message: string): Promise<AgentPlan> {
  return httpPost<AgentPlan>('/api/agent/plan', { message })
}

/** 直接调用单个业务工具（所有表单写操作统一入口） */
export async function callTool<T = ToolResult>(
  name: string,
  args: Record<string, unknown>,
): Promise<T> {
  return httpPost<T>('/api/tools/call', { name, args })
}

/** 工作台 KPI */
export async function getMetrics(): Promise<Metrics> {
  return httpGet<Metrics>('/api/metrics')
}

/** Agent 工具调用审计日志 */
export async function getAudit(limit = 100): Promise<AuditEvent[]> {
  const data = await httpGet<unknown>(`/api/audit?limit=${limit}`)
  return rowsFrom<AuditEvent>(data, 'events', 'audit')
}

/** 重置演示数据（保留账号与会话） */
export async function resetSeed(): Promise<{ ok: boolean }> {
  return httpPost<{ ok: boolean }>('/api/seed/reset', {})
}

/* ---------------- 附件 / 发票图片（统一上传通道） ---------------- */

export interface Attachment {
  id: string
  entity_type: string
  entity_id: string
  file_name: string
  mime: string
  size_bytes: number
  uploaded_by: string
  created_at: string
}

export interface AttachmentContent {
  id: string
  file_name: string
  mime: string
  size_bytes: number
  data_url: string
}

/** File → base64（去掉 `data:...;base64,` 前缀） */
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const url = String(reader.result || '')
      const idx = url.indexOf(',')
      if (idx < 0) reject(new Error('读取文件失败'))
      else resolve(url.slice(idx + 1))
    }
    reader.onerror = () => reject(reader.error || new Error('读取文件失败'))
    reader.readAsDataURL(file)
  })
}

/** 上传附件：服务端统一落盘并校验类型/大小/权限，客户端不保存本地路径 */
export async function uploadAttachment(
  file: File,
  entityType: string,
  entityId: string,
): Promise<Attachment> {
  const data = await fileToBase64(file)
  return httpPost<Attachment>('/api/files', {
    entity_type: entityType,
    entity_id: entityId,
    file_name: file.name,
    mime: file.type,
    data_base64: data,
  })
}

export async function listAttachments(entityType: string, entityId: string): Promise<Attachment[]> {
  const q = `entity_type=${encodeURIComponent(entityType)}&entity_id=${encodeURIComponent(entityId)}`
  const data = await httpGet<unknown>(`/api/files?${q}`)
  return rowsFrom<Attachment>(data, 'rows')
}

/** 读取附件内容并转成 data URL，预览/下载都不需要额外的静态文件服务 */
export async function readAttachment(id: string): Promise<AttachmentContent> {
  return httpGet<AttachmentContent>(`/api/files/${encodeURIComponent(id)}`)
}

export async function deleteAttachment(id: string): Promise<{ ok: boolean }> {
  return httpDelete<{ ok: boolean }>(`/api/files/${encodeURIComponent(id)}`)
}

/** 功能验收矩阵（与 domain::FEATURES 同一份常量） */
export async function getFeatures(): Promise<FeatureItem[]> {
  const data = await httpGet<unknown>('/api/features')
  return rowsFrom<FeatureItem>(data, 'features')
}
