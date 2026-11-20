/**
 * 全局共享状态：
 *  - auth：登录用户与角色权限（登录态由后端会话保证，前端仅做 UI 裁剪）
 *  - app / session：外壳与 Agent 会话（抽屉与全屏复用同一份响应式状态）
 *  - dataVersion：写操作成功后自增，所有业务视图 watch 它重新拉数据
 *  - sendAgent / previewPlan / submitTool：集中的 Agent 与工具调用逻辑
 */
import { computed, reactive, ref } from 'vue'
import { Modal, message } from 'ant-design-vue'
import {
  agentPlan as apiAgentPlan,
  agentStream,
  callTool,
  getToken,
  listAgentModels,
  listTools,
  type ChatTurn,
  login as apiLogin,
  logout as apiLogout,
  me as apiMe,
  type AgentPlan,
  type AgentReply,
  type AuthUser,
  type ThinkStep,
  type ToolResult,
  type ToolSpec,
  type TraceStep,
} from './api/bridge'
import {
  getBase,
  getMode,
  loadEndpoints,
  probe,
  resolveEndpoint,
  saveEndpoints,
  type ApiMode,
  type EndpointConfig,
} from './api/endpoint'
import { gt } from './i18n'

export type PageKey =
  | 'dashboard' | 'hr' | 'finance' | 'contract'
  | 'attendance' | 'asset' | 'agent' | 'arch'

/** 兼容旧视图的展示常量（后端只信任会话身份，这两个值仅用于本地提示文案） */
export const CURRENT_USER_ID = 'E01'
export const CURRENT_ACTOR = 'Evan'

export interface ChatMessage {
  id: number
  role: 'user' | 'agent'
  text: string
  time: string
  trace?: TraceStep[]
  mode?: string
  affected?: string[]
  /** 思考过程：流式时逐条追加，结束后保留供折叠查看 */
  thinking?: ThinkStep[]
  /** 该条回复仍在流式接收中 */
  streaming?: boolean
}

/* ---------------- 认证 ---------------- */

export const auth = reactive({
  /** 首次恢复登录态完成前为 false，外壳据此显示加载态 */
  ready: false,
  user: null as AuthUser | null,
  token: '',
  loading: false,
  error: '',
})

export const isAuthenticated = computed(() => !!auth.user)

/* ---------------- 服务端地址（局域网 / 外网，自动选路） ---------------- */

export const endpoint = reactive({
  lan: '',
  wan: '',
  mode: 'lan' as ApiMode,
  base: '',
  latencyMs: 0,
  /** 登录页「测试连接」进行中 */
  checking: false,
})

/** 启动时从 localStorage 回填，避免输入框空白 */
export function loadEndpointConfig(): void {
  const cfg = loadEndpoints()
  endpoint.lan = cfg.lan
  endpoint.wan = cfg.wan
  endpoint.mode = getMode()
  endpoint.base = getBase()
}
loadEndpointConfig()

/** 保存登录页输入的地址（不复位当前生效地址） */
export function persistEndpoints(lan = endpoint.lan, wan = endpoint.wan): EndpointConfig {
  const cfg = saveEndpoints({ lan, wan })
  endpoint.lan = cfg.lan
  endpoint.wan = cfg.wan
  return cfg
}

/** 登录页「测试连接」：两个地址各探测一次，只回结果，不改当前生效地址 */
export async function testEndpoints(): Promise<{ lan: boolean; wan: boolean }> {
  const cfg = persistEndpoints()
  endpoint.checking = true
  try {
    const lan = await probe(cfg.lan)
    const wan = cfg.wan && cfg.wan !== cfg.lan ? await probe(cfg.wan) : { ok: false }
    return { lan: lan.ok, wan: wan.ok }
  } finally {
    endpoint.checking = false
  }
}

/** 探测并切换到可用地址（LAN 优先 → WAN），返回是否成功 */
export async function selectEndpoint(): Promise<boolean> {
  const state = await resolveEndpoint({ lan: endpoint.lan, wan: endpoint.wan })
  if (!state) return false
  endpoint.mode = state.mode
  endpoint.base = state.base
  endpoint.latencyMs = state.latencyMs
  return true
}

/** 当前角色是否允许调用某个工具（后端仍会二次校验） */
export function can(tool: string): boolean {
  const u = auth.user
  if (!u) return false
  return u.role === 'admin' || u.permissions.includes(tool)
}

function errorText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e)
  if (raw.includes('UNAUTHORIZED')) return gt('auth.sessionExpired')
  const idx = raw.indexOf('): ')
  return idx >= 0 ? raw.slice(idx + 3) : raw
}

/**
 * 应用启动时恢复登录态：先按「局域网优先、外网兜底」重新选路，
 * 再用本地令牌换用户；两个地址都不可达时停在登录页，让用户改地址。
 */
export async function restoreAuth(): Promise<void> {
  try {
    loadEndpointConfig()
    // 未登录时不必探测，省掉一次 1.2s 等待
    if (getToken() && (await selectEndpoint())) {
      auth.user = await apiMe()
      if (auth.user) await loadTools()
    }
  } catch {
    auth.user = null
  } finally {
    auth.ready = true
  }
}

export async function doLogin(username: string, password: string): Promise<boolean> {
  if (auth.loading) return false
  auth.loading = true
  auth.error = ''
  try {
    // 1) 地址先落盘；2) 探测选路（LAN 优先，失败切 WAN）；3) 再登录
    persistEndpoints()
    if (!(await selectEndpoint())) {
      auth.error = gt('auth.noServer')
      return false
    }
    const s = await apiLogin(username.trim(), password)
    auth.user = s.user
    auth.token = s.token
    session.tools = []
    await loadTools()
    return true
  } catch (e) {
    auth.error = errorText(e)
    return false
  } finally {
    auth.loading = false
  }
}

export async function doLogout(): Promise<void> {
  try {
    await apiLogout()
  } catch {
    // 令牌可能已被改密或过期吊销，清理本地状态即可
  } finally {
    auth.user = null
    auth.token = ''
    clearSession()
  }
}

/* ---------------- 外壳 ---------------- */

export const app = reactive({
  activePage: 'dashboard' as PageKey,
  agentDrawer: false,
  drawerWide: false,
  returnPage: 'dashboard' as PageKey,
})

/** 写操作 / Agent 执行后自增，驱动所有业务视图刷新 */
export const dataVersion = ref(0)
export function bump(): void {
  dataVersion.value++
}

export const session = reactive({
  messages: [] as ChatMessage[],
  trace: [] as TraceStep[],
  running: false,
  tools: [] as ToolSpec[],
  plan: null as AgentPlan | null,
  planLoading: false,
  error: '',
  mode: '',
  /** 对话页选中的模型；空串表示沿用「模型设置」里存的那个 */
  model: '',
  /** 当前端点可选的模型清单，来自 GET /api/agent/models */
  models: [] as string[],
  modelsLoaded: false,
})

/** 拉取可选模型；失败只记一笔，不影响对话本身 */
export async function loadAgentModels(): Promise<void> {
  try {
    const res = await listAgentModels()
    session.models = res.models
    if (!session.model) session.model = res.current
    session.modelsLoaded = true
  } catch {
    // 端点未实现 /models 或无模型配置时静默降级：下拉为空即不显示
    session.modelsLoaded = true
  }
}

let msgId = 0
function clock(): string {
  const d = new Date()
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}
function pushMsg(
  role: 'user' | 'agent',
  text: string,
  extra: Partial<ChatMessage> = {},
): ChatMessage {
  session.messages.push({ id: ++msgId, role, text, time: clock(), ...extra })
  // 必须回读数组元素拿到响应式代理：直接返回刚 push 的字面量对象时，后续对它的
  // 修改不经过 Vue 的 Proxy，不会触发重渲染——表现就是流式内容"最后一次性冒出来"。
  return session.messages[session.messages.length - 1]
}

/* ---------------- Agent 会话 ---------------- */

/** 随每次请求带回的历史轮数上限，与服务端 `MAX_HISTORY_TURNS` 保持一致 */
const HISTORY_TURNS = 20

const SENSITIVE = [
  'hr.offboard', 'hr.promote', 'finance.claim.approve',
  'contract.create', 'contract.approve', 'attendance.approve',
]
const CONFIRM_KEYS: Record<string, string> = {
  'hr.offboard': 'offboard',
  'hr.promote': 'promote',
  'finance.claim.approve': 'approve',
  'contract.create': 'contract',
  'contract.approve': 'contractApprove',
  'attendance.approve': 'attendanceApprove',
}

function askConfirm(name: string): Promise<boolean> {
  return new Promise((resolve) => {
    Modal.confirm({
      title: gt('confirm.title'),
      content: gt('confirm.' + (CONFIRM_KEYS[name] || 'generic')),
      okText: gt('common.confirm'),
      cancelText: gt('common.cancel'),
      okType: 'danger',
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    })
  })
}

export async function sendAgent(text: string): Promise<void> {
  const cmd = String(text || '').trim()
  if (!cmd || session.running) return
  if (!auth.user) {
    session.error = gt('auth.sessionExpired')
    return
  }
  session.error = ''
  session.plan = null
  // 上下文：本轮之前的对话（只取文本）随请求提交，服务端不保存会话状态。
  // 必须在 push 本轮用户消息之前采样，否则当前问句会重复出现在历史里。
  const history: ChatTurn[] = session.messages
    .filter((m) => m.text.trim())
    .slice(-HISTORY_TURNS)
    .map((m) => ({ role: m.role, text: m.text }))
  pushMsg('user', cmd)
  session.running = true
  session.trace = []
  // 先占位：思考步骤每来一条就追加，回复帧到达后再补正文与轨迹
  const msg = pushMsg('agent', '', { thinking: [], streaming: true })
  try {
    // 用对象承接回调里的赋值：闭包内的赋值不参与 TS 的控制流收窄
    const holder: { reply: AgentReply | null } = { reply: null }
    let streamError = ''
    await agentStream(cmd, history, session.model, (event) => {
      if (event.type === 'think') {
        msg.thinking?.push({ stage: event.stage, text: event.text })
      } else if (event.type === 'delta') {
        // 正文增量：直接追加到占位消息，回复帧到达后再用完整文本覆盖
        msg.text += event.text
      } else if (event.type === 'reply') {
        holder.reply = event.reply
      } else {
        streamError = event.message
      }
    })
    if (streamError) throw new Error(streamError)
    const reply = holder.reply
    if (!reply) throw new Error(gt('agent.streamBroken'))

    const trace: TraceStep[] = Array.isArray(reply.trace) ? reply.trace : []
    msg.text = reply.reply || gt('agent.unknown')
    msg.trace = trace
    msg.mode = String(reply.mode || '')
    msg.affected = Array.isArray(reply.affected) ? reply.affected : []
    if (Array.isArray(reply.thinking) && reply.thinking.length) msg.thinking = reply.thinking
    msg.streaming = false
    session.trace = trace
    session.mode = msg.mode

    // 敏感操作门禁：Agent 停下后由用户确认，确认则带 confirmed=true 重试该步骤
    const pending = reply.pending
    if (pending && pending.tool) {
      const ok = await askConfirm(pending.tool)
      if (!ok) {
        pushMsg('agent', gt('agent.cancelled'))
        return
      }
      const result = await callTool<ToolResult>(pending.tool, { ...pending.args, confirmed: true })
      const step: TraceStep = {
        tool: pending.tool,
        args: pending.args,
        result,
        ok: !!result?.ok,
      }
      session.trace = [...session.trace, step]
      pushMsg('agent', String(result?.msg || result?.message || gt('toast.saved')), {
        trace: [step],
        mode: session.mode,
      })
    }
    bump() // Agent 可能写入了业务数据，刷新各台账
  } catch (e) {
    if (String(e).includes('UNAUTHORIZED')) auth.user = null
    session.error = errorText(e)
    msg.text = gt('error.requestFailed') + ': ' + errorText(e)
    msg.streaming = false
  } finally {
    session.running = false
  }
}

export async function previewPlan(text: string): Promise<void> {
  const cmd = String(text || '').trim()
  if (!cmd || session.planLoading) return
  session.planLoading = true
  session.error = ''
  try {
    session.plan = await apiAgentPlan(cmd)
  } catch (e) {
    if (String(e).includes('UNAUTHORIZED')) auth.user = null
    session.error = errorText(e)
    message.error(gt('error.requestFailed'))
  } finally {
    session.planLoading = false
  }
}

export function clearSession(): void {
  session.messages = []
  session.trace = []
  session.plan = null
  session.error = ''
  session.mode = ''
}

export async function loadTools(): Promise<void> {
  try {
    session.tools = await listTools()
  } catch {
    session.tools = []
  }
}

/* ---------------- 工具写操作（含敏感操作二次确认） ---------------- */

/**
 * 统一写操作入口：
 *  1) 四类敏感工具提交前先弹 Modal.confirm，确认后带 confirmed:true 提交；
 *  2) 若后端仍返回 needs_confirmation，则再次确认并带 confirmed:true 重发；
 *  3) 成功后 bump() 刷新所有台账，并用 message 反馈结果。
 */
export async function submitTool(
  name: string,
  args: Record<string, unknown>,
  _actor = CURRENT_ACTOR,
): Promise<ToolResult> {
  const sensitive = SENSITIVE.includes(name)
  // 调用方已在自己的弹窗里确认过（如审批意见框）就别再弹一次
  const preConfirmed = args.confirmed === true
  if (sensitive && !preConfirmed && !(await askConfirm(name))) {
    return { ok: false, cancelled: true }
  }
  const finalArgs = sensitive ? { ...args, confirmed: true } : args
  try {
    let result = await callTool<ToolResult>(name, finalArgs)
    if (result && result.needs_confirmation) {
      if (!(await askConfirm(name))) return result
      result = await callTool<ToolResult>(name, { ...args, confirmed: true })
    }
    if (result?.ok) {
      message.success(String(result.msg || gt('toast.saved')))
      bump()
    } else if (result && !result.cancelled) {
      message.warning(String(result.msg || gt('toast.failed')))
    }
    return result
  } catch (e) {
    if (String(e).includes('UNAUTHORIZED')) auth.user = null
    message.error(gt('error.requestFailed') + ': ' + errorText(e))
    return { ok: false, msg: errorText(e) }
  }
}

/* ---------------- 抽屉 / 全屏 / 全局指令 ---------------- */

export function openAgentDrawer(): void {
  if (app.activePage !== 'agent') app.returnPage = app.activePage
  app.agentDrawer = true
}
export function closeAgentDrawer(): void {
  app.agentDrawer = false
}
export function expandFullscreen(): void {
  app.agentDrawer = false
  app.activePage = 'agent'
}
export function dock(): void {
  app.activePage = app.returnPage || 'dashboard'
  app.agentDrawer = true
}
/** 全局搜索 / 快捷入口：唤起抽屉并把内容作为 Agent 指令发送 */
export function runCommand(cmd: string): void {
  const text = String(cmd || '').trim()
  openAgentDrawer()
  if (text) void sendAgent(text)
}
