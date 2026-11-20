/**
 * 领域视图模型：把后端台账行（snake_case / 金额分 / 双语对象）归一化成
 * 前端模板可直接使用的干净结构。字段名兼容 camelCase 与 snake_case 两套约定。
 */
import { L, num, pick, str, toYuan, type Row } from './utils'

export interface Department {
  id: string
  name: string
  parent: string
  leaderId: string
}
export function toDepartment(r: Row): Department {
  return {
    id: str(pick(r, 'id')),
    name: L(pick(r, 'name')),
    parent: str(pick(r, 'parent_id', 'parent', 'parentId')),
    leaderId: str(pick(r, 'leader_id', 'leaderId')),
  }
}

export interface Employee {
  id: string
  no: string
  name: string
  deptId: string
  position: string
  status: string
  entryDate: string
  salary: number // 元
  managerId: string
  email: string
}
export function toEmployee(r: Row): Employee {
  return {
    id: str(pick(r, 'id')),
    no: str(pick(r, 'no', 'emp_no', 'employee_no')),
    name: L(pick(r, 'name')),
    deptId: str(pick(r, 'dept_id', 'deptId', 'department_id')),
    position: L(pick(r, 'position')),
    status: str(pick(r, 'status')),
    entryDate: str(pick(r, 'entry_date', 'entryDate')),
    salary: toYuan(r, 'salary_cents', 'salary'),
    managerId: str(pick(r, 'manager_id', 'managerId')),
    email: str(pick(r, 'email')),
  }
}

export interface HrCase {
  id: string
  type: string
  empId: string
  effectiveDate: string
  reason: string
  status: string
  stepIndex: number
  steps: string
}
export function toHrCase(r: Row): HrCase {
  return {
    id: str(pick(r, 'id', 'case_no')),
    type: str(pick(r, 'type')),
    empId: str(pick(r, 'emp_id', 'empId', 'employee_id')),
    effectiveDate: str(pick(r, 'effective_date', 'effectiveDate')),
    reason: L(pick(r, 'reason')),
    status: str(pick(r, 'status')),
    stepIndex: num(pick(r, 'step_index', 'stepIndex')),
    steps: L(pick(r, 'steps')),
  }
}

export interface Claim {
  id: string
  type: string
  applicantId: string
  deptId: string
  amount: number // 元
  purpose: string
  project: string
  status: string
  submitDate: string
  approverId: string
}
export function toClaim(r: Row): Claim {
  return {
    id: str(pick(r, 'id')),
    // claims 表列名是 kind / submitted_at，少写这两个别名会让类型列渲染成「st.—」
    type: str(pick(r, 'kind', 'type')),
    applicantId: str(pick(r, 'applicant_id', 'applicantId')),
    deptId: str(pick(r, 'dept_id', 'deptId')),
    amount: toYuan(r, 'amount_cents', 'amount'),
    purpose: L(pick(r, 'purpose')),
    project: L(pick(r, 'project')),
    status: str(pick(r, 'status')),
    submitDate: str(pick(r, 'submitted_at', 'submit_date', 'submitDate')),
    approverId: str(pick(r, 'approver_id', 'approverId')),
  }
}

/**
 * 费用类型标准（分）：与 Rust `domain::CLAIM_ITEM_STANDARDS` 同源，
 * 前端只用于实时提示与提交前置校验，最终裁决仍在后端。unit 仅用于文案展示。
 */
export const CLAIM_ITEM_STANDARDS: Record<string, { cents: number; unit: 'time' | 'person' }> = {
  transport: { cents: 500000, unit: 'time' },
  hotel: { cents: 500000, unit: 'time' },
  meal: { cents: 100000, unit: 'person' },
  office: { cents: 300000, unit: 'time' },
  other: { cents: 500000, unit: 'time' },
}

/** 费用明细：一条一行，金额单位元；standardCents / overStandard 由后端台账补充 */
export interface ClaimItem {
  id: string
  claimId: string
  category: string
  amount: number
  occurredOn: string
  invoiceNo: string
  note: string
  standardCents: number
  overStandard: boolean
}
export function toClaimItem(r: Row): ClaimItem {
  const category = str(pick(r, 'category'))
  const amount = toYuan(r, 'amount_cents', 'amount')
  const standard = num(pick(r, 'standard_cents')) || CLAIM_ITEM_STANDARDS[category]?.cents || 0
  const flag = pick(r, 'over_standard')
  return {
    id: str(pick(r, 'id')),
    claimId: str(pick(r, 'claim_id', 'claimId')),
    category,
    amount,
    occurredOn: str(pick(r, 'occurred_on', 'occurredOn')),
    invoiceNo: str(pick(r, 'invoice_no', 'invoiceNo')),
    note: L(pick(r, 'note')),
    standardCents: standard,
    // 后端没给标记时按同一套标准兜底（离线/老接口）
    overStandard: flag == null ? standard > 0 && Math.round(amount * 100) > standard : num(flag) === 1,
  }
}

/** 审批链节点：approver 存的是具体员工号（按人员指定），不是角色名 */
export interface ApprovalFlow {
  id: string
  entityId: string
  step: number
  approver: string
  status: string // pending | done | rejected
  comment: string
  actedAt: string
}
export function toFlow(r: Row): ApprovalFlow {
  return {
    id: str(pick(r, 'id')),
    entityId: str(pick(r, 'entity_id', 'entityId')),
    step: num(pick(r, 'step')),
    approver: str(pick(r, 'approver')),
    status: str(pick(r, 'status')),
    comment: L(pick(r, 'comment')),
    actedAt: str(pick(r, 'acted_at', 'actedAt')),
  }
}

export interface Contract {
  id: string
  name: string
  party: string
  type: string
  amount: number // 元
  signDate: string
  expireDate: string
  status: string
  ownerId: string
}
export function toContract(r: Row): Contract {
  return {
    id: str(pick(r, 'id')),
    name: L(pick(r, 'name')),
    party: L(pick(r, 'party')),
    type: str(pick(r, 'type')),
    amount: toYuan(r, 'amount_cents', 'amount'),
    signDate: str(pick(r, 'sign_date', 'signDate')),
    expireDate: str(pick(r, 'expire_date', 'expireDate')),
    status: str(pick(r, 'status')),
    ownerId: str(pick(r, 'owner_id', 'ownerId')),
  }
}

export interface Attendance {
  id: string
  empId: string
  date: string
  checkIn: string
  checkOut: string
  status: string
  source: string
  hours: number
}
export function toAttendance(r: Row): Attendance {
  return {
    id: str(pick(r, 'id')),
    // attendance 表列名是 employee_id / work_date，少写这两个别名会让今日打卡与月度汇总全空
    empId: str(pick(r, 'employee_id', 'employeeId', 'emp_id', 'empId')),
    date: str(pick(r, 'work_date', 'workDate', 'date')),
    checkIn: str(pick(r, 'check_in', 'in', 'clock_in', 'checkIn')),
    checkOut: str(pick(r, 'check_out', 'out', 'clock_out', 'checkOut')),
    status: str(pick(r, 'status')),
    source: str(pick(r, 'source')),
    hours: num(pick(r, 'hours')),
  }
}

/** 补卡申请：申请只落请求，主管通过后才写 attendance */
export interface MakeupRequest {
  id: string
  empId: string
  date: string
  checkIn: string
  checkOut: string
  reason: string
  status: string // pending | approved | rejected
  approverId: string
  comment: string
  decidedAt: string
}
export function toMakeupRequest(r: Row): MakeupRequest {
  return {
    id: str(pick(r, 'id')),
    empId: str(pick(r, 'employee_id', 'employeeId')),
    date: str(pick(r, 'work_date', 'workDate', 'date')),
    checkIn: str(pick(r, 'clock_in', 'clockIn')),
    checkOut: str(pick(r, 'clock_out', 'clockOut')),
    reason: L(pick(r, 'reason')),
    status: str(pick(r, 'status')),
    approverId: str(pick(r, 'approver_id', 'approverId')),
    comment: L(pick(r, 'comment')),
    decidedAt: str(pick(r, 'decided_at', 'decidedAt')),
  }
}

export interface Asset {
  id: string
  category: string
  model: string
  sn: string
  status: string
  holderId: string
  location: string
  price: number // 元
  purchaseDate: string
}
export function toAsset(r: Row): Asset {
  return {
    id: str(pick(r, 'id')),
    category: str(pick(r, 'category')),
    model: L(pick(r, 'model')),
    sn: str(pick(r, 'sn')),
    status: str(pick(r, 'status')),
    holderId: str(pick(r, 'holder_id', 'holderId')),
    location: L(pick(r, 'location')),
    price: toYuan(r, 'price_cents', 'price'),
    purchaseDate: str(pick(r, 'purchase_date', 'purchaseDate')),
  }
}

/* ---------------- 静态业务数据（双语，作为 listTools 失败时兜底） ---------------- */

export interface Bilingual {
  zh: string
  en: string
}

export interface ToolEntry {
  name: string
  domain: string
  params: string
  desc: Bilingual
}

/** 19 个业务工具协议（listTools 失败时的离线兜底，与 Rust TOOL_SPECS 一致） */
export const TOOL_CATALOG: ToolEntry[] = [
  { name: 'hr.onboard', domain: 'hr', params: 'name, dept, position, entry_date, salary, manager', desc: { zh: '创建员工档案并启动入职流程（资料→合同→设备→培训）', en: 'Create employee record and kick off the onboarding flow' } },
  { name: 'hr.offboard', domain: 'hr', params: 'employee, last_day, reason, handover_to', desc: { zh: '发起离职流程、冻结账号、生成交接与资产回收清单', en: 'Start offboarding, freeze accounts, build handover & asset return checklist' } },
  { name: 'hr.promote', domain: 'hr', params: 'employee, new_salary, new_position, effective_date, reason', desc: { zh: '晋升/涨薪，写入异动记录并按金额分派审批链', en: 'Promotion / raise: write HR change record and route approval chain' } },
  { name: 'hr.roster.query', domain: 'hr', params: 'dept?, status?, keyword?', desc: { zh: '按部门、状态、关键词检索员工花名册', en: 'Query the employee roster by department, status or keyword' } },
  { name: 'org.chart', domain: 'org', params: 'dept?', desc: { zh: '返回组织架构与各节点实时在编人数', en: 'Return the org chart with live headcount per node' } },
  { name: 'finance.claim.create', domain: 'finance', params: 'type, applicant, amount, purpose, project?, items?', desc: { zh: '创建三类报销单，自动校验预算并生成审批链', en: 'Create daily / business / project claims with budget check and approval routing' } },
  { name: 'finance.claim.approve', domain: 'finance', params: 'claim_id, decision, comment?', desc: { zh: '审批通过 / 驳回报销单，通过后进入付款队列', en: 'Approve or reject a claim; approved claims enter the payment queue' } },
  { name: 'finance.summary', domain: 'finance', params: 'month?, dept?, group_by', desc: { zh: '按部门 / 类型汇总报销金额与预算执行率', en: 'Aggregate claim amounts by department / type and budget usage' } },
  { name: 'contract.create', domain: 'contract', params: 'name, party, type, amount, sign_date, expire_date, owner', desc: { zh: '创建合同台账并发起用印 / 法务审批', en: 'Create a contract record and start legal / seal approval' } },
  { name: 'contract.approve', domain: 'contract', params: 'contract_id, decision, comment?', desc: { zh: '合同法务 / 用印逐级审批，末级通过后生效', en: 'Approve or reject a contract step by step; effective after the final step' } },
  { name: 'contract.query', domain: 'contract', params: 'keyword?, status?, expiring_in_days?', desc: { zh: '检索合同并输出到期预警清单', en: 'Search contracts and surface expiry alerts' } },
  { name: 'attendance.checkin', domain: 'attendance', params: 'employee, type(in|out), time?, source?', desc: { zh: '写入上班 / 下班打卡记录，自动判定迟到早退', en: 'Record clock-in / clock-out and detect late / early anomalies' } },
  { name: 'attendance.makeup', domain: 'attendance', params: 'employee, date, reason, clock_in?, clock_out?', desc: { zh: '提交补卡申请（同人同日限一条待审），进入主管审批', en: 'Submit a punch-makeup request for supervisor approval' } },
  { name: 'attendance.approve', domain: 'attendance', params: 'request_id, decision, comment?', desc: { zh: '补卡审批：通过后才补入考勤，驳回不写考勤', en: 'Approve a makeup request; attendance is written only after approval' } },
  { name: 'attendance.report', domain: 'attendance', params: 'month, dept?, group_by', desc: { zh: '生成月度考勤报表：按人汇总出勤率/迟到/早退/缺卡/外勤/工时', en: 'Monthly attendance report per employee: rate, late, early, missing, field, hours' } },
  { name: 'asset.register', domain: 'asset', params: 'category, model, sn, price, location', desc: { zh: '登记新资产并生成资产编号与二维码台账', en: 'Register a new asset, generate asset ID and inventory record' } },
  { name: 'asset.assign', domain: 'asset', params: 'asset_id, employee, note?', desc: { zh: '将资产领用给员工并写入责任人', en: 'Assign an asset to an employee and record custody' } },
  { name: 'asset.return', domain: 'asset', params: 'asset_id, condition?', desc: { zh: '归还资产，状态回写为在库并释放责任人', en: 'Return an asset, reset to in-stock and release custody' } },
  { name: 'asset.query', domain: 'asset', params: 'category?, status?, keyword?', desc: { zh: '资产盘点：按类别 / 状态 / 领用人检索', en: 'Inventory lookup by category, status or holder' } },
]

export interface QuickItem {
  label: Bilingual
  cmd: Bilingual
}
export interface QuickGroup {
  key: string
  icon: string
  label: Bilingual
  items: QuickItem[]
}

/** 6 个业务域，每域 1-2 条中英文指令 */
export const QUICK_GROUPS: QuickGroup[] = [
  {
    key: 'hr', icon: 'users', label: { zh: '人事', en: 'HR' },
    items: [
      { label: { zh: '查询花名册', en: 'Roster' }, cmd: { zh: '查询研发中心在职员工花名册', en: 'List active employees in R&D Center' } },
      { label: { zh: '办理入职', en: 'Onboard' }, cmd: { zh: '为许知远办理入职，研发中心 / 前端工程师，11月18日报到，月薪 18000', en: 'Onboard Xu Zhiyuan as Frontend Engineer in R&D Center, starting Nov 18, salary 18000' } },
    ],
  },
  {
    key: 'finance', icon: 'wallet', label: { zh: '财务', en: 'Finance' },
    items: [
      { label: { zh: '商务报销', en: 'Business claim' }, cmd: { zh: '提交一笔商务报销：申请人陈静，金额 3280 元，事由客户拜访差旅', en: 'Submit a business expense claim: applicant Chen Jing, amount 3280 CNY, client visit travel' } },
      { label: { zh: '费用统计', en: 'Cost report' }, cmd: { zh: '统计本月各部门报销金额', en: 'Summarize claim amounts by department this month' } },
    ],
  },
  {
    key: 'contract', icon: 'file', label: { zh: '合同', en: 'Contracts' },
    items: [
      { label: { zh: '到期预警', en: 'Expiry check' }, cmd: { zh: '查询即将到期的合同', en: 'List contracts expiring soon' } },
      { label: { zh: '新建合同', en: 'New contract' }, cmd: { zh: '新建合同：与恒信科技签订服务器采购合同，金额 48 万，2026-11-20 签署', en: 'Create a contract: server purchase with Hengxin Technology, value 480000 CNY, signed 2026-11-20' } },
    ],
  },
  {
    key: 'attendance', icon: 'clock', label: { zh: '考勤', en: 'Attendance' },
    items: [
      { label: { zh: '上班打卡', en: 'Clock in' }, cmd: { zh: '帮我打上班卡', en: 'Clock me in for work' } },
      { label: { zh: '月度报表', en: 'Monthly report' }, cmd: { zh: '生成 11 月考勤报表', en: 'Generate the November attendance report' } },
    ],
  },
  {
    key: 'asset', icon: 'box', label: { zh: '资产', en: 'Assets' },
    items: [
      { label: { zh: '资产领用', en: 'Assign' }, cmd: { zh: '把 AST-0008 领用给刘洋', en: 'Assign asset AST-0008 to Liu Yang' } },
      { label: { zh: '资产盘点', en: 'Inventory' }, cmd: { zh: '盘点在库的电脑类资产', en: 'Inventory all in-stock computer assets' } },
    ],
  },
  {
    key: 'insight', icon: 'layers', label: { zh: '综合洞察', en: 'Insights' },
    items: [
      { label: { zh: '组织架构', en: 'Org chart' }, cmd: { zh: '返回公司组织架构与各部门实时在编人数', en: 'Show the org chart with live headcount per department' } },
      { label: { zh: '资产归还', en: 'Return' }, cmd: { zh: '归还资产 AST-0007', en: 'Return asset AST-0007' } },
    ],
  },
]

/** 五端打包矩阵 */
export const PLATFORMS = [
  { key: 'win', icon: 'monitor', color: 'blue', cmd: 'npm run tauri build -- --target x86_64-pc-windows-msvc' },
  { key: 'mac', icon: 'laptop', color: 'purple', cmd: 'npm run tauri build -- --target aarch64-apple-darwin' },
  { key: 'linux', icon: 'code', color: 'cyan', cmd: 'npm run tauri build -- --target x86_64-unknown-linux-gnu' },
  { key: 'android', icon: 'phone', color: 'green', cmd: 'npm run tauri android init && npm run tauri android build --aab' },
  { key: 'ios', icon: 'phone', color: 'orange', cmd: 'npm run tauri ios init && npm run tauri ios build' },
]

/** 技术栈卡片 */
export const STACK_CARDS = [
  { key: 'vue', ico: 'code', color: 'green', title: 'Vue 3' },
  { key: 'i18n', ico: 'globe', color: 'cyan', title: 'vue-i18n 9' },
  { key: 'antd', ico: 'grid', color: 'blue', title: 'Ant Design Vue 4' },
  { key: 'rust', ico: 'cpu', color: 'orange', title: 'Rust 1.97+' },
  { key: 'rig', ico: 'bot', color: 'purple', title: 'Rig 0.46' },
  { key: 'tauri', ico: 'layers', color: 'blue', title: 'Tauri 2.0' },
  { key: 'db', ico: 'database', color: 'green', title: 'SQLite / PostgreSQL' },
  { key: 'llm', ico: 'sparkle', color: 'cyan', title: 'Pluggable LLM' },
]

/** 分层架构图 */
export const ARCH_LAYERS = [
  { k: 'l1', ico: 'grid', pills: ['Vue SFC', 'Pinia', 'Design Token', 'vue-i18n'] },
  { k: 'l2', ico: 'flow', pills: ['Tauri invoke', 'Axum HTTP', 'bridge.ts', 'Capability'] },
  { k: 'l3', ico: 'bot', pills: ['Rig AgentBuilder', 'Tool trait', 'AgentHook', 'ToolContext'] },
  { k: 'l4', ico: 'database', pills: ['Approval FSM', 'Audit Event', 'sqlx', 'Migration'] },
]

export const SECURITY_KEYS = ['sec1', 'sec2', 'sec3', 'sec4', 'sec5']

/* ---------------- 常用筛选选项 ---------------- */
export const ASSET_CATEGORIES = ['laptop', 'desktop', 'printer', 'usb', 'disc', 'monitor', 'projector', 'other']
export const CLAIM_TYPES = ['daily', 'business', 'project']
export const CONTRACT_TYPES = ['sales', 'purchase', 'labor', 'service', 'nda']
export const EMP_STATUSES = ['active', 'probation', 'leaving', 'left']
export const CLAIM_STATUSES = ['draft', 'pending', 'approved', 'rejected', 'paid']
export const ATTENDANCE_STATUSES = ['normal', 'late', 'early', 'missing', 'field', 'leave']
