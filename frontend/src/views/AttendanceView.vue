<script setup lang="ts">
/** 考勤：今日打卡 / 打卡·补卡表单 / 月度汇总 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import { listTable } from '../api/bridge'
import {
  ATTENDANCE_STATUSES, toAttendance, toDepartment, toEmployee, toFlow, toMakeupRequest,
  type ApprovalFlow, type Attendance, type Department, type Employee, type MakeupRequest,
} from '../model'
import { nowTime, tagColor, today } from '../utils'
import { auth, CURRENT_ACTOR, CURRENT_USER_ID, can, dataVersion, submitTool } from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const attendance = ref<Attendance[]>([])
const employees = ref<Employee[]>([])
const departments = ref<Department[]>([])
const makeups = ref<MakeupRequest[]>([])
const flows = ref<ApprovalFlow[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [at, em, de, mk, fl] = await Promise.all([
      listTable('attendance'), listTable('employees'), listTable('departments'),
      listTable('makeup_requests'), listTable('approval_flows'),
    ])
    attendance.value = at.map(toAttendance)
    employees.value = em.map(toEmployee)
    departments.value = de.map(toDepartment)
    makeups.value = mk.map(toMakeupRequest)
    flows.value = fl.map(toFlow)
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
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
const summaryRows = computed(() =>
  ATTENDANCE_STATUSES.map((k) => ({ key: k, n: monthRows.value.filter((a) => a.status === k).length })),
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
  for (const a of monthRows.value) {
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
  { title: t('attendance.colName'), key: 'name' },
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
  { title: t('attendance.colName'), key: 'name' },
  { title: t('attendance.colDept'), key: 'dept' },
  { title: t('attendance.colIn'), dataIndex: 'checkIn', key: 'in' },
  { title: t('attendance.colOut'), dataIndex: 'checkOut', key: 'out' },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colSource'), key: 'source' },
  { title: t('attendance.colHours'), dataIndex: 'hours', key: 'hours', align: 'right' as const },
])

/* ---------- 打卡 / 补卡 / 报表 ---------- */
const punchEmployee = ref('')
async function punch(type: 'in' | 'out'): Promise<void> {
  const employeeId = punchEmployee.value || auth.user?.employee_id || employees.value[0]?.id || CURRENT_USER_ID
  await submitTool('attendance.checkin', {
    employeeId, type, source: 'desktop', time: nowTime(),
  }, CURRENT_ACTOR)
  punchEmployee.value = employeeId
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
    }, CURRENT_ACTOR)
    if (res.ok) decideOpen.value = false
  } finally {
    deciding.value = false
  }
}

const makeupColumns = computed(() => [
  { title: t('attendance.makeupRequest'), dataIndex: 'id', key: 'id' },
  { title: t('attendance.colName'), key: 'name' },
  { title: t('attendance.colDate'), dataIndex: 'date', key: 'date' },
  { title: t('attendance.colIn'), key: 'in' },
  { title: t('attendance.colOut'), key: 'out' },
  { title: t('attendance.makeupReason'), dataIndex: 'reason', key: 'reason' },
  { title: t('attendance.colStatus'), key: 'status' },
  { title: t('attendance.colApprover'), key: 'approver' },
  { title: t('common.action'), key: 'action' },
])

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
    }, CURRENT_ACTOR)
    if (res.ok) makeupOpen.value = false
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('attendance.title') }}</h1><p>{{ t('attendance.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button type="primary" @click="openMakeup"><Icon name="plus" :size="14" /> {{ t('attendance.makeup') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="punch-card mb16">
        <div>
          <div class="punch-time">{{ clock }}</div>
          <div class="punch-date">{{ latestDate }} · {{ t('header.today') }}</div>
        </div>
        <div class="punch-stat">
          <div v-for="p in punchStats" :key="p.label"><b>{{ p.v }}</b><span>{{ p.label }}</span></div>
        </div>
        <span class="spacer"></span>
        <a-select v-model:value="punchEmployee" :options="empOptions" :placeholder="t('attendance.selectEmployee')" style="width: 180px" />
        <button class="punch-btn" @click="punch('in')"><Icon name="clock" :size="14" /> {{ t('attendance.checkinNow') }}</button>
        <button class="punch-btn ghost" @click="punch('out')">{{ t('attendance.punchOut') }}</button>
      </div>

      <div class="oa-grid g-2-1">
        <a-card :title="t('attendance.punchedAt')">
          <template #extra><span class="small muted">{{ latestDate }}</span></template>
          <a-table class="oa-table" :columns="columns" :data-source="todayRows" row-key="id" :pagination="false">
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
          <span class="small muted">{{ t('attendance.reportDesc') }} · {{ reportMonth }}</span>
        </template>
        <a-table
          class="oa-table"
          :columns="reportColumns"
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
          <span class="small muted">{{ t('attendance.pendingMakeups') }} {{ pendingMakeups }}</span>
        </template>
        <a-table
          class="oa-table"
          :columns="makeupColumns"
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
    </AsyncPanel>

    <!-- 补卡申请 -->
    <a-modal v-model:open="makeupOpen" :title="t('attendance.makeupTitle')" :confirm-loading="submitting" @ok="submitMakeup">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · attendance.makeup'" />
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
  </div>
</template>
