<script setup lang="ts">
/** 考勤：今日打卡 / 打卡·补卡表单 / 月度汇总 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import ColumnPicker from '../components/ColumnPicker.vue'
import { useColumns } from '../composables/useColumns'
import {
  getAttendancePolicy, getMakeupQuota, listTable,
  type AttendancePolicy, type MakeupQuotaInfo,
} from '../api/bridge'
import {
  ATTENDANCE_STATUSES, LEAVE_TYPES, toAttendance, toAttendanceExemption, toDepartment,
  toEmployee, toFlow, toLeaveBalance, toLeaveBalanceLog, toLeaveRequest, toMakeupRequest,
  toOvertimeRequest,
  type ApprovalFlow, type Attendance, type AttendanceExemption, type Department, type Employee,
  type LeaveBalance, type LeaveBalanceLog, type LeaveRequest, type MakeupRequest,
  type OvertimeRequest,
} from '../model'
import { nowTime, tagColor, today } from '../utils'
import { getCurrentLocation, GeoError, type GeoPoint } from '../composables/useGeolocation'
import { auth, can, dataVersion, submitTool, takePageFilter } from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const attendance = ref<Attendance[]>([])
const employees = ref<Employee[]>([])
const departments = ref<Department[]>([])
const makeups = ref<MakeupRequest[]>([])
const leaveRequests = ref<LeaveRequest[]>([])
const overtimes = ref<OvertimeRequest[]>([])
const balances = ref<LeaveBalance[]>([])
const balanceLogs = ref<LeaveBalanceLog[]>([])
const makeupQuota = ref<MakeupQuotaInfo | null>(null)
const attendancePolicy = ref<AttendancePolicy | null>(null)
const selfExempt = ref(false)
const exemptions = ref<AttendanceExemption[]>([])
const flows = ref<ApprovalFlow[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [at, em, de, mk, fl, lv, lb, ot, bl, mq, pol, ex] = await Promise.all([
      listTable('attendance'), listTable('employees'), listTable('departments'),
      listTable('makeup_requests'), listTable('approval_flows'),
      listTable('leave_requests'), listTable('leave_balances'),
      listTable('overtime_requests'), listTable('leave_balance_logs'),
      getMakeupQuota().catch(() => null),
      getAttendancePolicy().catch(() => null),
      listTable('attendance_exemptions').catch(() => []),
    ])
    attendance.value = at.map(toAttendance)
    employees.value = em.map(toEmployee)
    departments.value = de.map(toDepartment)
    makeups.value = mk.map(toMakeupRequest)
    flows.value = fl.map(toFlow)
    leaveRequests.value = lv.map(toLeaveRequest)
    balances.value = lb.map(toLeaveBalance)
    overtimes.value = ot.map(toOvertimeRequest)
    balanceLogs.value = bl.map(toLeaveBalanceLog)
    makeupQuota.value = mq
    attendancePolicy.value = pol?.policy ?? null
    selfExempt.value = pol?.exempt_self ?? false
    exemptions.value = ex.map(toAttendanceExemption)
    // 当前登录人可能不在可打卡名单里（如已离职），回落到第一个可选员工
    if (!punchEmployee.value) punchEmployee.value = empOptions.value[0]?.value || ''
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
// 工作台「今日出勤率」卡片带过来的筛选：把月度报表定位到当月（今日汇总在页首）
const pageFilter = takePageFilter('attendance')
onMounted(() => {
  if (pageFilter.attendanceMonth) reportMonth.value = pageFilter.attendanceMonth
  void load()
})
watch(dataVersion, load)

/* ---------- 时钟 ---------- */
const clock = ref(nowTime())
let timer: ReturnType<typeof setInterval> | undefined
onMounted(() => { timer = setInterval(() => { clock.value = nowTime() }, 15000) })
onUnmounted(() => { if (timer) clearInterval(timer) })

function empName(id: string): string {
  return employees.value.find((e) => e.id === id)?.name || '—'
}
function deptOf(id: string): string {
  const d = departments.value.find((x) => x.id === employees.value.find((e) => e.id === id)?.deptId)
  return d?.name || '—'
}
const empOptions = computed(() =>
  employees.value
    .filter((e) => e.status !== 'left')
    .filter((e) => auth.user?.role !== 'employee' || e.id === auth.user?.employee_id)
    .map((e) => ({ label: `${e.name} · ${e.no}`, value: e.id })),
)

/** 以台账中最新日期作为「今日」，兼容 seed 日期与运行时日期不一致的情况 */
const latestDate = computed(() => {
  const dates = attendance.value.map((a) => a.date).filter((d) => d && d !== '—').sort()
  return dates.length ? dates[dates.length - 1] : today()
})
const todayRows = computed(() => attendance.value.filter((a) => a.date === latestDate.value))
const punchStats = computed(() => [
  { label: t('attendance.todayNormal'), v: todayRows.value.filter((a) => a.status === 'normal').length },
  { label: t('attendance.todayLate'), v: todayRows.value.filter((a) => a.status === 'late').length },
  { label: t('attendance.todayLeave'), v: todayRows.value.filter((a) => a.status === 'field' || a.status === 'leave').length },
])
/** 报表月份：取数据里出现过的月份，默认最近一个月 */
const months = computed(() => {
  const set = new Set(attendance.value.map((a) => a.date.slice(0, 7)).filter((m) => m && m !== '—'))
  return [...set].sort().reverse()
})
const reportMonth = ref('')
watch(months, (list) => {
  if (list.length && !list.includes(reportMonth.value)) reportMonth.value = list[0]
}, { immediate: true })

const monthRows = computed(() =>
  attendance.value.filter((a) => a.date.slice(0, 7) === (reportMonth.value || latestDate.value.slice(0, 7))),
)
/** 免考勤员工集合：员工级直接命中，部门级展开到该部门所有员工 */
const exemptEmpIds = computed(() => {
  const ids = new Set<string>()
  for (const e of exemptions.value) {
    if (e.scope === 'employee') {
      ids.add(e.targetId)
    } else if (e.scope === 'department') {
      for (const emp of employees.value) {
        if (emp.deptId === e.targetId) ids.add(emp.id)
      }
    }
  }
  return ids
})
/** 参与考勤统计的考勤行：排除免考勤人员 */
const reportableMonthRows = computed(() =>
  monthRows.value.filter((a) => !exemptEmpIds.value.has(a.empId)),
)
const summaryRows = computed(() =>
  ATTENDANCE_STATUSES.map((k) => ({
    key: k,
    n: reportableMonthRows.value.filter((a) => a.status === k).length,
  })),
)

interface ReportRow {
  empId: string
  dept: string
  days: number
  normal: number
  late: number
  early: number
  missing: number
  field: number
  leave: number
  hours: number
  rate: number
}
/** 与后端 attendance.report 同一口径：按人汇总，出勤率 =（正常 + 外勤）/ 出勤天数 */
const reportRows = computed<ReportRow[]>(() => {
  const map = new Map<string, ReportRow>()
  for (const a of reportableMonthRows.value) {
    const row = map.get(a.empId) ?? {
      empId: a.empId, dept: deptOf(a.empId),
      days: 0, normal: 0, late: 0, early: 0, missing: 0, field: 0, leave: 0, hours: 0, rate: 0,
    }
    row.days += 1
    if (a.status === 'normal') row.normal += 1
    else if (a.status === 'late') row.late += 1
    else if (a.status === 'early') row.early += 1
    else if (a.status === 'missing') row.missing += 1
    else if (a.status === 'field') row.field += 1
    else if (a.status === 'leave') row.leave += 1
    row.hours += Number(a.hours) || 0
    map.set(a.empId, row)
  }
  const rows = [...map.values()].sort((x, y) => y.late - x.late || y.days - x.days)
  for (const r of rows) {
    r.hours = Math.round(r.hours * 10) / 10
    r.rate = r.days ? Math.round(((r.normal + r.field) / r.days) * 1000) / 10 : 0
  }
  return rows
})
const monthRate = computed(() => {
  const days = reportRows.value.reduce((n, r) => n + r.days, 0)
  const good = reportRows.value.reduce((n, r) => n + r.normal + r.field, 0)
  return days ? Math.round((good / days) * 1000) / 10 : 0
})

const reportColumns = computed(() => [
  { title: t('attendance.colName'), key: 'name', locked: true },
  { title: t('attendance.colDept'), dataIndex: 'dept', key: 'dept' },
  { title: t('attendance.colDays'), dataIndex: 'days', key: 'days', align: 'right' as const },
  { title: t('attendance.colNormal'), dataIndex: 'normal', key: 'normal', align: 'right' as const },
  { title: t('attendance.colLate'), dataIndex: 'late', key: 'late', align: 'right' as const },
  { title: t('attendance.colEarly'), dataIndex: 'early', key: 'early', align: 'right' as const },
  { title: t('attendance.colMissing'), dataIndex: 'missing', key: 'missing', align: 'right' as const },
  { title: t('attendance.colField'), dataIndex: 'field', key: 'field', align: 'right' as const },
  { title: t('attendance.colLeave'), dataIndex: 'leave', key: 'leave', align: 'right' as const },
  { title: t('attendance.colHours'), dataIndex: 'hours', key: 'hours', align: 'right' as const },
  { title: t('attendance.colRate'), key: 'rate', align: 'right' as const },
])
const columns = computed(() => [
  { title: t('attendance.colName'), key: 'name', locked: true },
  { title: t('attendance.colDept'), key: 'dept' },
  { title: t('attendance.colIn'), dataIndex: 'checkIn', key: 'in' },
  { title: t('attendance.colOut'), dataIndex: 'checkOut', key: 'out' },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colSource'), key: 'source' },
  { title: t('attendance.colHours'), dataIndex: 'hours', key: 'hours', align: 'right' as const },
])

/* ---------- 打卡 / 补卡 / 报表 ---------- */
/**
 * 默认选中当前登录人：空字符串会让 a-select 渲染一个空选中项盖住 placeholder，
 * 看起来像「下拉框没有数据」；有值时打卡对象也一目了然。
 */
const punchEmployee = ref(auth.user?.employee_id || '')
const locating = ref(false)
const geoError = ref('')
const fieldPunch = ref(false)
const fieldNote = ref('')
const mobileClient = /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent)

/**
 * 打卡前先定位：普通打卡拿不到定位就中止；外勤打卡允许没有定位（客户现场信号差），
 * 有定位时仍会记录坐标与到打卡点的距离。
 */
async function punch(type: 'in' | 'out'): Promise<void> {
  const employeeId = punchEmployee.value || auth.user?.employee_id || employees.value[0]?.id || ''
  locating.value = true
  geoError.value = ''
  let point: GeoPoint | null = null
  try {
    point = await getCurrentLocation()
  } catch (e) {
    if (!fieldPunch.value) {
      locating.value = false
      geoError.value = e instanceof GeoError ? t(`attendance.geo.${e.kind}`) : String(e)
      message.error(geoError.value)
      return
    }
    // 外勤：没有定位也允许提交
  }
  try {
    const note = fieldPunch.value ? fieldNote.value.trim() : ''
    const res = await submitTool('attendance.checkin', {
      employeeId,
      type,
      source: mobileClient ? 'mobile' : 'desktop',
      time: nowTime(),
      field: fieldPunch.value,
      ...(note ? { note } : {}),
      ...(point ? { latitude: point.latitude, longitude: point.longitude } : {}),
    })
    if (res.ok) punchEmployee.value = employeeId
    else if (!res.cancelled) geoError.value = String(res.msg || '')
  } catch (e) {
    geoError.value = e instanceof GeoError ? t(`attendance.geo.${e.kind}`) : String(e)
    message.error(geoError.value)
  } finally {
    locating.value = false
  }
}
/* ---------- 补卡审批（申请只落请求，通过后才写考勤） ---------- */
function flowsOf(id: string): ApprovalFlow[] {
  return flows.value.filter((f) => f.entityId === id).sort((a, b) => a.step - b.step)
}
function currentNode(id: string): ApprovalFlow | undefined {
  return flowsOf(id).find((f) => f.status === 'pending')
}
function isMyTurn(row: MakeupRequest): boolean {
  if (row.status !== 'pending' || !can('attendance.approve')) return false
  if (auth.user?.role === 'admin') return true
  return currentNode(row.id)?.approver === auth.user?.employee_id
}
const pendingMakeups = computed(() => makeups.value.filter((m) => m.status === 'pending').length)

const decideOpen = ref(false)
const decideTarget = ref<MakeupRequest | null>(null)
const decideKind = ref<'approve' | 'reject'>('approve')
const decideComment = ref('')
const deciding = ref(false)

function openDecide(row: MakeupRequest, kind: 'approve' | 'reject'): void {
  decideTarget.value = row
  decideKind.value = kind
  decideComment.value = ''
  decideOpen.value = true
}

async function submitDecide(): Promise<void> {
  const row = decideTarget.value
  if (!row) return
  deciding.value = true
  try {
    // 弹窗本身即确认，带 confirmed 跳过 submitTool 的二次确认
    const res = await submitTool('attendance.approve', {
      request_id: row.id, decision: decideKind.value, comment: decideComment.value.trim(), confirmed: true,
    })
    if (res.ok) decideOpen.value = false
  } finally {
    deciding.value = false
  }
}

const makeupColumns = computed(() => [
  { title: t('attendance.makeupRequest'), dataIndex: 'id', key: 'id', locked: true },
  { title: t('attendance.colName'), key: 'name' },
  { title: t('attendance.colDate'), dataIndex: 'date', key: 'date' },
  { title: t('attendance.colIn'), key: 'in' },
  { title: t('attendance.colOut'), key: 'out' },
  { title: t('attendance.makeupReason'), dataIndex: 'reason', key: 'reason' },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colApprover'), key: 'approver' },
  { title: t('common.action'), key: 'action', locked: true },
])
const todayModel = useColumns('attendance.today', () => columns.value, () => auth.user?.username || '')
const reportModel = useColumns('attendance.report', () => reportColumns.value, () => auth.user?.username || '')
const makeupModel = useColumns('attendance.makeup', () => makeupColumns.value, () => auth.user?.username || '')

const makeupOpen = ref(false)
const submitting = ref(false)
const makeupForm = reactive({ employeeId: '', date: today(), clockIn: '09:00', clockOut: '18:00', reason: '' })
function openMakeup(): void {
  // 默认给当前登录人：普通员工的下拉里只有自己
  const me = auth.user?.employee_id || ''
  const options = empOptions.value
  const employeeId = options.some((o) => o.value === me) ? me : (options[0]?.value || '')
  Object.assign(makeupForm, {
    employeeId, date: today(), clockIn: '09:00', clockOut: '18:00', reason: '',
  })
  makeupOpen.value = true
  // 打开弹窗时刷新本月次数，避免用过期数据判断能否提交
  void getMakeupQuota().then((q) => { makeupQuota.value = q }).catch(() => {})
}
async function submitMakeup(): Promise<void> {
  submitting.value = true
  try {
    const res = await submitTool('attendance.makeup', {
      employeeId: makeupForm.employeeId,
      date: makeupForm.date || today(),
      clockIn: makeupForm.clockIn,
      clockOut: makeupForm.clockOut,
      reason: makeupForm.reason.trim() || '—',
    })
    if (res.ok) makeupOpen.value = false
  } finally {
    submitting.value = false
  }
}

/* ---------- 请假申请 / 审批 / 撤销 / 额度 ---------- */
const leaveOpen = ref(false)
const leaveSubmitting = ref(false)
const leaveForm = reactive({
  employeeId: '',
  leaveType: 'annual',
  startDate: today(),
  endDate: today(),
  reason: '',
})
const leaveTypeOptions = computed(() =>
  LEAVE_TYPES.map((k) => ({ label: t(`leaveType.${k}`), value: k })),
)
const myBalances = computed(() =>
  balances.value.filter((b) => b.empId === (auth.user?.employee_id || '')),
)
function balanceOf(empId: string, leaveType: string): LeaveBalance | undefined {
  return balances.value.find((b) => b.empId === empId && b.leaveType === leaveType)
}
const pendingLeaves = computed(() => leaveRequests.value.filter((l) => l.status === 'pending').length)

function openLeave(): void {
  const me = auth.user?.employee_id || ''
  const options = empOptions.value
  const employeeId = options.some((o) => o.value === me) ? me : (options[0]?.value || '')
  Object.assign(leaveForm, {
    employeeId, leaveType: 'annual', startDate: today(), endDate: today(), reason: '',
  })
  leaveOpen.value = true
}

async function submitLeave(): Promise<void> {
  leaveSubmitting.value = true
  try {
    const res = await submitTool('attendance.leave', {
      employeeId: leaveForm.employeeId,
      leaveType: leaveForm.leaveType,
      startDate: leaveForm.startDate,
      endDate: leaveForm.endDate,
      reason: leaveForm.reason.trim() || '—',
    })
    if (res.ok) leaveOpen.value = false
  } finally {
    leaveSubmitting.value = false
  }
}

function isLeaveTurn(row: LeaveRequest): boolean {
  if (row.status !== 'pending' || !can('attendance.leave.approve')) return false
  if (auth.user?.role === 'admin') return true
  return row.approverId === auth.user?.employee_id
}
function canCancelLeave(row: LeaveRequest): boolean {
  return row.status === 'pending'
    && (auth.user?.role === 'admin' || row.empId === auth.user?.employee_id)
}

const leaveDecideOpen = ref(false)
const leaveDecideTarget = ref<LeaveRequest | null>(null)
const leaveDecideKind = ref<'approve' | 'reject'>('approve')
const leaveDecideComment = ref('')
const leaveDeciding = ref(false)

function openLeaveDecide(row: LeaveRequest, kind: 'approve' | 'reject'): void {
  leaveDecideTarget.value = row
  leaveDecideKind.value = kind
  leaveDecideComment.value = ''
  leaveDecideOpen.value = true
}

async function submitLeaveDecide(): Promise<void> {
  const row = leaveDecideTarget.value
  if (!row) return
  leaveDeciding.value = true
  try {
    const res = await submitTool('attendance.leave.approve', {
      leave_id: row.id,
      decision: leaveDecideKind.value,
      comment: leaveDecideComment.value.trim(),
      confirmed: true,
    })
    if (res.ok) leaveDecideOpen.value = false
  } finally {
    leaveDeciding.value = false
  }
}

async function cancelLeave(row: LeaveRequest): Promise<void> {
  await submitTool('attendance.leave.cancel', { leave_id: row.id })
}

const balanceOpen = ref(false)
const balanceSaving = ref(false)
const balanceForm = reactive({ employeeId: '', leaveType: 'annual', totalDays: 5 })
const balanceTypeOptions = computed(() =>
  LEAVE_TYPES.filter((k) => k === 'annual' || k === 'compensatory')
    .map((k) => ({ label: t(`leaveType.${k}`), value: k })),
)

function openBalance(): void {
  Object.assign(balanceForm, {
    employeeId: empOptions.value[0]?.value || '',
    leaveType: 'annual',
    totalDays: 5,
  })
  balanceOpen.value = true
}

async function saveBalance(): Promise<void> {
  balanceSaving.value = true
  try {
    const res = await submitTool('hr.leave.balance.set', {
      employee_id: balanceForm.employeeId,
      leave_type: balanceForm.leaveType,
      total_days: balanceForm.totalDays,
    })
    if (res.ok) balanceOpen.value = false
  } finally {
    balanceSaving.value = false
  }
}

const leaveColumns = computed(() => [
  { title: t('attendance.leaveId'), dataIndex: 'id', key: 'id' },
  { title: t('attendance.colName'), key: 'name' },
  { title: t('attendance.leaveTypeCol'), key: 'type' },
  { title: t('attendance.leaveRange'), key: 'range' },
  { title: t('attendance.leaveDays'), dataIndex: 'days', key: 'days', align: 'right' as const },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colApprover'), key: 'approver' },
  { title: t('common.action'), key: 'action' },
])

/* ---------- 加班申请 / 审批 / 撤销 ---------- */
const overtimeOpen = ref(false)
const overtimeSubmitting = ref(false)
const overtimeForm = reactive({
  employeeId: '',
  workDate: today(),
  // 需求默认值：18:00 开始
  startTime: '18:00',
  endTime: '21:00',
  reason: '',
})
const pendingOvertimes = computed(() => overtimes.value.filter((o) => o.status === 'pending').length)
const myBalanceLogs = computed(() =>
  balanceLogs.value
    .filter((l) => l.empId === (auth.user?.employee_id || ''))
    .slice(0, 10),
)

function openOvertime(): void {
  const me = auth.user?.employee_id || ''
  const options = empOptions.value
  const employeeId = options.some((o) => o.value === me) ? me : (options[0]?.value || '')
  Object.assign(overtimeForm, {
    employeeId, workDate: today(), startTime: '18:00', endTime: '21:00', reason: '',
  })
  overtimeOpen.value = true
}

async function submitOvertime(): Promise<void> {
  overtimeSubmitting.value = true
  try {
    const res = await submitTool('attendance.overtime', {
      employeeId: overtimeForm.employeeId,
      workDate: overtimeForm.workDate,
      startTime: overtimeForm.startTime,
      endTime: overtimeForm.endTime,
      reason: overtimeForm.reason.trim() || '—',
    })
    if (res.ok) overtimeOpen.value = false
  } finally {
    overtimeSubmitting.value = false
  }
}

function isOvertimeTurn(row: OvertimeRequest): boolean {
  if (row.status !== 'pending' || !can('attendance.overtime.approve')) return false
  if (auth.user?.role === 'admin') return true
  return row.approverId === auth.user?.employee_id
}
function canCancelOvertime(row: OvertimeRequest): boolean {
  return row.status === 'pending'
    && (auth.user?.role === 'admin' || row.empId === auth.user?.employee_id)
}

const overtimeDecideOpen = ref(false)
const overtimeDecideTarget = ref<OvertimeRequest | null>(null)
const overtimeDecideKind = ref<'approve' | 'reject'>('approve')
const overtimeDecideComment = ref('')
const overtimeDeciding = ref(false)

function openOvertimeDecide(row: OvertimeRequest, kind: 'approve' | 'reject'): void {
  overtimeDecideTarget.value = row
  overtimeDecideKind.value = kind
  overtimeDecideComment.value = ''
  overtimeDecideOpen.value = true
}

async function submitOvertimeDecide(): Promise<void> {
  const row = overtimeDecideTarget.value
  if (!row) return
  overtimeDeciding.value = true
  try {
    const res = await submitTool('attendance.overtime.approve', {
      overtime_id: row.id,
      decision: overtimeDecideKind.value,
      comment: overtimeDecideComment.value.trim(),
      confirmed: true,
    })
    if (res.ok) overtimeDecideOpen.value = false
  } finally {
    overtimeDeciding.value = false
  }
}

async function cancelOvertime(row: OvertimeRequest): Promise<void> {
  await submitTool('attendance.overtime.cancel', { overtime_id: row.id })
}

/** 天数展示：整数不带小数，非整数最多 3 位（0.375 天不会被截成 0.4） */
function fmtDays(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(3)))
}

const overtimeColumns = computed(() => [
  { title: t('attendance.overtimeId'), dataIndex: 'id', key: 'id' },
  { title: t('attendance.colName'), key: 'name' },
  { title: t('attendance.overtimeDate'), dataIndex: 'workDate', key: 'date' },
  { title: t('attendance.overtimeRange'), key: 'range' },
  { title: t('attendance.overtimeHours'), key: 'hours' },
  { title: t('attendance.overtimeCredited'), key: 'credited' },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colApprover'), key: 'approver' },
  { title: t('common.action'), key: 'action' },
])
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('attendance.title') }}</h1><p>{{ t('attendance.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button type="primary" @click="openLeave"><Icon name="plus" :size="14" /> {{ t('attendance.leave') }}</a-button>
        <a-button @click="openOvertime"><Icon name="plus" :size="14" /> {{ t('attendance.overtime') }}</a-button>
        <a-button @click="openMakeup"><Icon name="plus" :size="14" /> {{ t('attendance.makeup') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="punch-card mb16">
        <div>
          <div class="punch-time">{{ clock }}</div>
          <div class="punch-date">
            {{ latestDate }} · {{ t('header.today') }}
            <span v-if="attendancePolicy"> · {{ attendancePolicy.work_start }}-{{ attendancePolicy.work_end }}</span>
          </div>
          <div v-if="selfExempt" class="punch-exempt">{{ t('attendance.exemptSelf') }}</div>
        </div>
        <div class="punch-stat">
          <div v-for="p in punchStats" :key="p.label"><b>{{ p.v }}</b><span>{{ p.label }}</span></div>
        </div>
        <span class="spacer"></span>
        <a-select v-model:value="punchEmployee" :options="empOptions" :placeholder="t('attendance.selectEmployee')" style="width: 180px" />
        <a-checkbox v-model:checked="fieldPunch" class="punch-field">{{ t('attendance.fieldPunch') }}</a-checkbox>
        <a-input
          v-if="fieldPunch"
          v-model:value="fieldNote"
          :placeholder="t('attendance.fieldNotePh')"
          style="width: 150px"
        />
        <button class="punch-btn" :disabled="locating" @click="punch('in')">
          <Icon name="clock" :size="14" />
          {{ locating ? t('attendance.geoLocating') : t('attendance.checkinNow') }}
        </button>
        <button class="punch-btn ghost" :disabled="locating" @click="punch('out')">{{ t('attendance.punchOut') }}</button>
      </div>
      <div v-if="geoError" class="punch-geo-error">{{ geoError }}</div>

      <div class="oa-grid g-2-1">
        <a-card :title="t('attendance.punchedAt')">
          <template #extra>
            <div style="display: flex; align-items: center; gap: 8px">
              <span class="small muted">{{ latestDate }}</span>
              <ColumnPicker :model="todayModel" />
            </div>
          </template>
          <a-table class="oa-table" :columns="todayModel.columns" :data-source="todayRows" row-key="id" :pagination="false">
            <template #bodyCell="{ column, record }">
              <template v-if="column.key === 'name'">{{ empName(record.empId) }}</template>
              <template v-else-if="column.key === 'dept'">{{ deptOf(record.empId) }}</template>
              <template v-else-if="column.key === 'in'"><span class="mono">{{ record.checkIn }}</span></template>
              <template v-else-if="column.key === 'out'"><span class="mono">{{ record.checkOut }}</span></template>
              <template v-else-if="column.key === 'status'"><a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag></template>
              <template v-else-if="column.key === 'source'"><a-tag>{{ t('attendance.source.' + record.source) }}</a-tag></template>
              <template v-else-if="column.key === 'hours'"><span class="cell-num">{{ record.hours || '—' }}</span></template>
            </template>
          </a-table>
        </a-card>

        <a-card :title="t('attendance.monthlyTitle')">
          <template #extra>
            <a-select
              v-model:value="reportMonth"
              :options="months.map((m) => ({ label: m, value: m }))"
              size="small"
              style="width: 108px"
            />
          </template>
          <div style="margin-bottom: 16px">
            <div style="display: flex; justify-content: space-between; font-size: 12.5px; margin-bottom: 6px">
              <span>{{ t('attendance.attendanceRate') }}</span><b>{{ monthRate }}%</b>
            </div>
            <a-progress :percent="Math.round(monthRate)" :stroke-width="12" />
          </div>
          <div v-for="r in summaryRows" :key="r.key" style="margin-bottom: 12px">
            <div style="display: flex; justify-content: space-between; font-size: 12.5px; margin-bottom: 5px">
              <span><a-tag :color="tagColor(r.key)">{{ t('st.' + r.key) }}</a-tag></span><b>{{ r.n }}</b>
            </div>
            <a-progress :percent="Math.round(r.n / Math.max(1, monthRows.length) * 100)" :stroke-color="r.key === 'normal' ? '#52c41a' : undefined" />
          </div>
        </a-card>
      </div>

      <!-- 月度考勤报表：按人汇总，与 attendance.report 同口径 -->
      <a-card class="mt16" :title="t('attendance.reportTitle')">
        <template #extra>
          <div style="display: flex; align-items: center; gap: 8px">
            <span class="small muted">{{ t('attendance.reportDesc') }} · {{ reportMonth }}</span>
            <ColumnPicker :model="reportModel" />
          </div>
        </template>
        <a-table
          class="oa-table"
          :columns="reportModel.columns"
          :data-source="reportRows"
          row-key="empId"
          :pagination="{ pageSize: 8 }"
          :locale="{ emptyText: t('attendance.reportEmpty') }"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">{{ empName(record.empId) }}</template>
            <template v-else-if="column.key === 'late'">
              <span :style="{ color: record.late ? '#d48806' : '' }">{{ record.late }}</span>
            </template>
            <template v-else-if="column.key === 'missing'">
              <span :style="{ color: record.missing ? '#cf1322' : '' }">{{ record.missing }}</span>
            </template>
            <template v-else-if="column.key === 'rate'"><b>{{ record.rate }}%</b></template>
          </template>
        </a-table>
      </a-card>

      <!-- 补卡申请：申请只落请求，主管通过后才补入考勤 -->
      <a-card class="mt16" :title="t('attendance.makeupList')">
        <template #extra>
          <div style="display: flex; align-items: center; gap: 8px">
            <span class="small muted">{{ t('attendance.pendingMakeups') }} {{ pendingMakeups }}</span>
            <ColumnPicker :model="makeupModel" />
          </div>
        </template>
        <a-table
          class="oa-table"
          :columns="makeupModel.columns"
          :data-source="makeups"
          row-key="id"
          :pagination="{ pageSize: 8 }"
          :locale="{ emptyText: t('attendance.makeupEmpty') }"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">{{ empName(record.empId) }}</template>
            <template v-else-if="column.key === 'in'"><span class="mono">{{ record.checkIn }}</span></template>
            <template v-else-if="column.key === 'out'"><span class="mono">{{ record.checkOut }}</span></template>
            <template v-else-if="column.key === 'status'">
              <a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag>
            </template>
            <template v-else-if="column.key === 'approver'">{{ empName(record.approverId) }}</template>
            <template v-else-if="column.key === 'action'">
              <div class="row-actions">
                <a-button v-if="isMyTurn(record)" size="small" type="primary" ghost @click="openDecide(record, 'approve')">
                  {{ t('attendance.approve') }}
                </a-button>
                <a-button v-if="isMyTurn(record)" size="small" danger @click="openDecide(record, 'reject')">
                  {{ t('attendance.reject') }}
                </a-button>
                <span v-if="!isMyTurn(record)" class="small muted">
                  {{ record.status === 'pending' ? t('attendance.waiting') : '—' }}
                </span>
              </div>
            </template>
          </template>
        </a-table>
      </a-card>

      <!-- 请假申请：申请时预占年假/调休额度，审批通过后写入考勤 -->
      <a-card class="mt16" :title="t('attendance.leaveList')">
        <template #extra>
          <div style="display: flex; align-items: center; gap: 8px">
            <span v-if="myBalances.length" class="small muted">
              <span v-for="b in myBalances" :key="b.leaveType" style="margin-left: 10px">
                {{ t(`leaveType.${b.leaveType}`) }} {{ b.availableDays.toFixed(1) }}/{{ b.totalDays.toFixed(1) }}
              </span>
            </span>
            <span class="small muted">{{ t('attendance.pendingLeaves') }} {{ pendingLeaves }}</span>
            <a-button v-if="can('hr.leave.balance.set')" size="small" @click="openBalance">
              {{ t('attendance.balanceEdit') }}
            </a-button>
          </div>
        </template>
        <a-table
          class="oa-table"
          :columns="leaveColumns"
          :data-source="leaveRequests"
          row-key="id"
          :pagination="{ pageSize: 8 }"
          :locale="{ emptyText: t('attendance.leaveEmpty') }"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">{{ empName(record.empId) }}</template>
            <template v-else-if="column.key === 'type'"><a-tag>{{ t(`leaveType.${record.leaveType}`) }}</a-tag></template>
            <template v-else-if="column.key === 'range'"><span class="mono">{{ record.startDate }} ~ {{ record.endDate }}</span></template>
            <template v-else-if="column.key === 'status'">
              <a-tag :color="tagColor(record.status)">{{ t('leaveSt.' + record.status) }}</a-tag>
            </template>
            <template v-else-if="column.key === 'approver'">{{ empName(record.approverId) }}</template>
            <template v-else-if="column.key === 'action'">
              <div class="row-actions">
                <a-button v-if="isLeaveTurn(record)" size="small" type="primary" ghost @click="openLeaveDecide(record, 'approve')">
                  {{ t('attendance.approve') }}
                </a-button>
                <a-button v-if="isLeaveTurn(record)" size="small" danger @click="openLeaveDecide(record, 'reject')">
                  {{ t('attendance.reject') }}
                </a-button>
                <a-button v-if="canCancelLeave(record)" size="small" @click="cancelLeave(record)">
                  {{ t('attendance.leaveCancel') }}
                </a-button>
                <span v-if="!isLeaveTurn(record) && !canCancelLeave(record)" class="small muted">
                  {{ record.status === 'pending' ? t('attendance.waiting') : '—' }}
                </span>
              </div>
            </template>
          </template>
        </a-table>

        <!-- 本人额度变动流水：加班转入、请假预占/回滚、HR 调整 -->
        <div v-if="myBalanceLogs.length" class="mt16">
          <div class="small muted mb8">{{ t('attendance.balanceLog') }}</div>
          <div v-for="l in myBalanceLogs" :key="l.id" class="desc-item">
            <span class="desc-k">{{ l.createdAt }}</span>
            <span class="desc-v">
              {{ t(`leaveType.${l.leaveType}`) }}
              <b :style="{ color: l.deltaDays >= 0 ? '#16a34a' : '#e5484d' }">
                {{ l.deltaDays >= 0 ? '+' : '' }}{{ fmtDays(l.deltaDays) }}
              </b>
              <span class="small muted"> · {{ l.reason }}</span>
            </span>
          </div>
        </div>
      </a-card>

      <!-- 加班申请：通过后按 8h = 1 天转入调休额度 -->
      <a-card class="mt16" :title="t('attendance.overtimeList')">
        <template #extra>
          <div style="display: flex; align-items: center; gap: 8px">
            <span class="small muted">{{ t('attendance.pendingOvertimes') }} {{ pendingOvertimes }}</span>
          </div>
        </template>
        <a-table
          class="oa-table"
          :columns="overtimeColumns"
          :data-source="overtimes"
          row-key="id"
          :pagination="{ pageSize: 8 }"
          :locale="{ emptyText: t('attendance.overtimeEmpty') }"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">{{ empName(record.empId) }}</template>
            <template v-else-if="column.key === 'date'"><span class="mono">{{ record.workDate }}</span></template>
            <template v-else-if="column.key === 'range'"><span class="mono">{{ record.startTime }} ~ {{ record.endTime }}</span></template>
            <template v-else-if="column.key === 'hours'"><span class="cell-num">{{ record.hours }} h</span></template>
            <template v-else-if="column.key === 'credited'">
              <span class="cell-num">{{ record.creditedDays ? '+' + fmtDays(record.creditedDays) + ' 天' : '—' }}</span>
            </template>
            <template v-else-if="column.key === 'status'">
              <a-tag :color="tagColor(record.status)">{{ t('leaveSt.' + record.status) }}</a-tag>
            </template>
            <template v-else-if="column.key === 'approver'">{{ empName(record.approverId) }}</template>
            <template v-else-if="column.key === 'action'">
              <div class="row-actions">
                <a-button v-if="isOvertimeTurn(record)" size="small" type="primary" ghost @click="openOvertimeDecide(record, 'approve')">
                  {{ t('attendance.approve') }}
                </a-button>
                <a-button v-if="isOvertimeTurn(record)" size="small" danger @click="openOvertimeDecide(record, 'reject')">
                  {{ t('attendance.reject') }}
                </a-button>
                <a-button v-if="canCancelOvertime(record)" size="small" @click="cancelOvertime(record)">
                  {{ t('attendance.leaveCancel') }}
                </a-button>
                <span v-if="!isOvertimeTurn(record) && !canCancelOvertime(record)" class="small muted">
                  {{ record.status === 'pending' ? t('attendance.waiting') : '—' }}
                </span>
              </div>
            </template>
          </template>
        </a-table>
      </a-card>
    </AsyncPanel>

    <!-- 补卡申请 -->
    <a-modal
      v-model:open="makeupOpen"
      :title="t('attendance.makeupTitle')"
      :confirm-loading="submitting"
      :ok-button-props="{ disabled: !!makeupQuota && makeupQuota.remaining <= 0 }"
      @ok="submitMakeup"
    >
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · attendance.makeup'" />
      <p v-if="makeupQuota" class="small muted">
        {{ t('attendance.makeupQuotaHint', { used: makeupQuota.used, quota: makeupQuota.quota, remaining: makeupQuota.remaining }) }}
      </p>
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')"><a-select v-model:value="makeupForm.employeeId" :options="empOptions" /></a-form-item>
          <a-form-item :label="t('attendance.makeupDate')"><a-date-picker v-model:value="makeupForm.date" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('attendance.makeupIn')"><a-input v-model:value="makeupForm.clockIn" placeholder="09:00" /></a-form-item>
          <a-form-item :label="t('attendance.makeupOut')"><a-input v-model:value="makeupForm.clockOut" placeholder="18:00" /></a-form-item>
          <a-form-item :label="t('attendance.makeupReason')"><a-input v-model:value="makeupForm.reason" /></a-form-item>
        </div>
      </a-form>
    </a-modal>

    <!-- 补卡审批（弹窗本身即确认） -->
    <a-modal v-model:open="decideOpen" :title="t('attendance.approveTitle')" :confirm-loading="deciding" @ok="submitDecide">
      <div class="small muted mb8" v-if="decideTarget">
        {{ decideTarget.id }} · {{ empName(decideTarget.empId) }} · {{ decideTarget.date }}
      </div>
      <a-alert
        v-if="decideTarget"
        :type="decideKind === 'reject' ? 'warning' : 'info'"
        show-icon
        class="form-hint"
        :message="t('attendance.flowTitle') + ' · ' + t('attendance.myTurn')"
      />
      <a-textarea v-model:value="decideComment" :placeholder="t('attendance.commentPh')" :rows="3" />
    </a-modal>

    <!-- 请假申请 -->
    <a-modal v-model:open="leaveOpen" :title="t('attendance.leaveTitle')" :confirm-loading="leaveSubmitting" @ok="submitLeave">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · attendance.leave'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')">
            <a-select v-model:value="leaveForm.employeeId" :options="empOptions" />
          </a-form-item>
          <a-form-item :label="t('attendance.leaveTypeCol')">
            <a-select v-model:value="leaveForm.leaveType" :options="leaveTypeOptions" />
          </a-form-item>
          <a-form-item :label="t('attendance.leaveStart')">
            <a-date-picker v-model:value="leaveForm.startDate" value-format="YYYY-MM-DD" style="width: 100%" />
          </a-form-item>
          <a-form-item :label="t('attendance.leaveEnd')">
            <a-date-picker v-model:value="leaveForm.endDate" value-format="YYYY-MM-DD" style="width: 100%" />
          </a-form-item>
          <a-form-item :label="t('attendance.leaveReason')">
            <a-input v-model:value="leaveForm.reason" />
          </a-form-item>
        </div>
        <p class="small muted">
          {{ t('attendance.leaveBalanceHint') }}
          <template v-if="balanceOf(leaveForm.employeeId, leaveForm.leaveType)">
            {{ t(`leaveType.${leaveForm.leaveType}`) }}
            {{ balanceOf(leaveForm.employeeId, leaveForm.leaveType)?.availableDays.toFixed(1) }}
            / {{ balanceOf(leaveForm.employeeId, leaveForm.leaveType)?.totalDays.toFixed(1) }}
          </template>
        </p>
      </a-form>
    </a-modal>

    <!-- 请假审批 -->
    <a-modal
      v-model:open="leaveDecideOpen"
      :title="t('attendance.leaveApproveTitle')"
      :confirm-loading="leaveDeciding"
      @ok="submitLeaveDecide"
    >
      <div class="small muted mb8" v-if="leaveDecideTarget">
        {{ leaveDecideTarget.id }} · {{ empName(leaveDecideTarget.empId) }} ·
        {{ leaveDecideTarget.startDate }} ~ {{ leaveDecideTarget.endDate }} ·
        {{ leaveDecideTarget.days }} {{ t('attendance.leaveDaysUnit') }}
      </div>
      <a-alert
        v-if="leaveDecideTarget"
        :type="leaveDecideKind === 'reject' ? 'warning' : 'info'"
        show-icon
        class="form-hint"
        :message="t('attendance.flowTitle') + ' · ' + t('attendance.myTurn')"
      />
      <a-textarea v-model:value="leaveDecideComment" :placeholder="t('attendance.commentPh')" :rows="3" />
    </a-modal>

    <!-- 年假/调休额度设置（HR / 管理员） -->
    <a-modal
      v-model:open="balanceOpen"
      :title="t('attendance.balanceTitle')"
      :confirm-loading="balanceSaving"
      @ok="saveBalance"
    >
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')">
            <a-select v-model:value="balanceForm.employeeId" :options="empOptions" />
          </a-form-item>
          <a-form-item :label="t('attendance.leaveTypeCol')">
            <a-select v-model:value="balanceForm.leaveType" :options="balanceTypeOptions" />
          </a-form-item>
          <a-form-item :label="t('attendance.balanceTotal')">
            <a-input-number v-model:value="balanceForm.totalDays" :min="0" :step="0.5" style="width: 100%" />
          </a-form-item>
        </div>
      </a-form>
    </a-modal>

    <!-- 加班申请（开始时间默认 18:00） -->
    <a-modal
      v-model:open="overtimeOpen"
      :title="t('attendance.overtimeTitle')"
      :confirm-loading="overtimeSubmitting"
      @ok="submitOvertime"
    >
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · attendance.overtime'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')">
            <a-select v-model:value="overtimeForm.employeeId" :options="empOptions" />
          </a-form-item>
          <a-form-item :label="t('attendance.overtimeDate')">
            <a-date-picker v-model:value="overtimeForm.workDate" value-format="YYYY-MM-DD" style="width: 100%" />
          </a-form-item>
          <a-form-item :label="t('attendance.overtimeStart')">
            <a-input v-model:value="overtimeForm.startTime" placeholder="18:00" />
          </a-form-item>
          <a-form-item :label="t('attendance.overtimeEnd')">
            <a-input v-model:value="overtimeForm.endTime" placeholder="21:00" />
          </a-form-item>
          <a-form-item :label="t('attendance.overtimeReason')">
            <a-input v-model:value="overtimeForm.reason" />
          </a-form-item>
        </div>
        <p class="small muted">{{ t('attendance.overtimeHint') }}</p>
      </a-form>
    </a-modal>

    <!-- 加班审批 -->
    <a-modal
      v-model:open="overtimeDecideOpen"
      :title="t('attendance.overtimeApproveTitle')"
      :confirm-loading="overtimeDeciding"
      @ok="submitOvertimeDecide"
    >
      <div class="small muted mb8" v-if="overtimeDecideTarget">
        {{ overtimeDecideTarget.id }} · {{ empName(overtimeDecideTarget.empId) }} ·
        {{ overtimeDecideTarget.workDate }} {{ overtimeDecideTarget.startTime }} ~
        {{ overtimeDecideTarget.endTime }} · {{ overtimeDecideTarget.hours }}h
      </div>
      <a-alert
        v-if="overtimeDecideTarget"
        :type="overtimeDecideKind === 'reject' ? 'warning' : 'info'"
        show-icon
        class="form-hint"
        :message="t('attendance.flowTitle') + ' · ' + t('attendance.myTurn')"
      />
      <a-textarea v-model:value="overtimeDecideComment" :placeholder="t('attendance.commentPh')" :rows="3" />
    </a-modal>
  </div>
</template>
