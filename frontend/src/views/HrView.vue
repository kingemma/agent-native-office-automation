<script setup lang="ts">
/** 人事：花名册 / 组织架构 / 异动记录 + 入职·离职·晋升表单 + 详情抽屉 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import { listTable } from '../api/bridge'
import {
  EMP_STATUSES, toDepartment, toEmployee, toHrCase,
  type Department, type Employee, type HrCase,
} from '../model'
import {
  aliased, centsArgs, dateAdd, money, tagColor, today,
} from '../utils'
import { can, dataVersion, submitTool } from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const employees = ref<Employee[]>([])
const departments = ref<Department[]>([])
const cases = ref<HrCase[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [em, de, ca] = await Promise.all([
      listTable('employees'),
      listTable('departments'),
      listTable('hr_cases'),
    ])
    employees.value = em.map(toEmployee)
    departments.value = de.map(toDepartment)
    cases.value = ca.map(toHrCase)
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
watch(dataVersion, load)

/* ---------- 筛选 ---------- */
const tab = ref<'roster' | 'org' | 'cases'>('roster')
const deptFilter = ref('')
const statusFilter = ref('')
const keyword = ref('')
const deptOptions = computed(() => departments.value.map((d) => ({ label: d.name, value: d.id })))
const empOptions = computed(() =>
  employees.value.filter((e) => e.status !== 'left').map((e) => ({ label: `${e.name} · ${e.no}`, value: e.id })),
)
const statusOptions = computed(() => EMP_STATUSES.map((s) => ({ label: t('st.' + s), value: s })))
const filteredEmployees = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return employees.value.filter((e) => {
    if (deptFilter.value && e.deptId !== deptFilter.value) return false
    if (statusFilter.value && e.status !== statusFilter.value) return false
    if (!kw) return true
    return `${e.name} ${e.no} ${e.position} ${e.email}`.toLowerCase().includes(kw)
  })
})

function deptName(id: string): string {
  return departments.value.find((d) => d.id === id)?.name || '—'
}
function empName(id: string): string {
  return employees.value.find((e) => e.id === id)?.name || '—'
}
function deptLeader(id: string): string {
  return departments.value.find((d) => d.id === id)?.leaderId || 'E01'
}
function headcount(id: string): number {
  return employees.value.filter((e) => e.deptId === id && e.status !== 'left').length
}
const avgTenure = computed(() => {
  const list = employees.value.filter((e) => e.status !== 'left')
  if (!list.length) return '0.0'
  const total = list.reduce((n, e) => {
    const d = new Date(e.entryDate)
    return n + (isNaN(d.getTime()) ? 0 : (Date.now() - d.getTime()) / 31536000000)
  }, 0)
  return (total / list.length).toFixed(1)
})
const hrKpis = computed(() => [
  { label: t('hr.rosterTotal'), value: String(employees.value.length), ico: 'users', color: 'blue' },
  { label: t('hr.rosterActive'), value: String(employees.value.filter((e) => e.status === 'active').length), ico: 'checked', color: 'green' },
  { label: t('hr.rosterProbation'), value: String(employees.value.filter((e) => e.status === 'probation').length), ico: 'clock', color: 'orange' },
  { label: t('hr.rosterAvg'), value: avgTenure.value + t('hr.years'), ico: 'trend', color: 'purple' },
])

/* ---------- 表格列 ---------- */
const rosterColumns = computed(() => [
  { title: t('hr.colNo'), dataIndex: 'no', key: 'no' },
  { title: t('hr.colName'), dataIndex: 'name', key: 'name' },
  { title: t('hr.colDept'), key: 'dept' },
  { title: t('hr.colPosition'), dataIndex: 'position', key: 'position' },
  { title: t('hr.colStatus'), key: 'status' },
  { title: t('hr.colEntry'), dataIndex: 'entryDate', key: 'entry' },
  { title: t('hr.colSalary'), key: 'salary', align: 'right' as const },
  { title: t('hr.colManager'), key: 'manager' },
  { title: t('common.action'), key: 'action' },
])
const caseColumns = computed(() => [
  { title: t('hr.caseNo'), dataIndex: 'id', key: 'id' },
  { title: t('hr.caseType'), key: 'type' },
  { title: t('hr.caseEmployee'), key: 'employee' },
  { title: t('hr.caseEffective'), dataIndex: 'effectiveDate', key: 'effective' },
  { title: t('hr.caseReason'), dataIndex: 'reason', key: 'reason' },
  { title: t('hr.caseStatus'), key: 'status' },
  { title: t('hr.caseSteps'), key: 'steps' },
  { title: t('common.action'), key: 'action' },
])

/* ---------- 详情抽屉 ---------- */
const detailOpen = ref(false)
const detailKind = ref<'employee' | 'case'>('employee')
const detailRow = ref<Employee | HrCase | null>(null)
const detailTitle = computed(() =>
  detailKind.value === 'employee' ? t('hr.detailTitle') : t('hr.caseDetailTitle'),
)
const detailRows = computed<{ k: string; v: string }[]>(() => {
  const r = detailRow.value
  if (!r) return []
  if (detailKind.value === 'employee') {
    const e = r as Employee
    return [
      { k: t('hr.colNo'), v: e.no }, { k: t('hr.colName'), v: e.name },
      { k: t('hr.colDept'), v: deptName(e.deptId) }, { k: t('hr.colPosition'), v: e.position },
      { k: t('hr.colStatus'), v: t('st.' + e.status) }, { k: t('hr.colEntry'), v: e.entryDate },
      { k: t('hr.colSalary'), v: money(e.salary) }, { k: t('hr.colManager'), v: empName(e.managerId) },
      { k: t('hr.colEmail'), v: e.email },
    ]
  }
  const c = r as HrCase
  return [
    { k: t('hr.caseNo'), v: c.id }, { k: t('hr.caseType'), v: t('st.' + c.type) },
    { k: t('hr.caseEmployee'), v: empName(c.empId) }, { k: t('hr.caseEffective'), v: c.effectiveDate },
    { k: t('hr.caseReason'), v: c.reason }, { k: t('hr.caseStatus'), v: t('st.' + c.status) },
    { k: t('hr.caseSteps'), v: `${c.stepIndex} / 5` }, { k: t('common.detail'), v: c.steps },
  ]
})
function openDetail(kind: 'employee' | 'case', row: Employee | HrCase): void {
  detailKind.value = kind
  detailRow.value = row
  detailOpen.value = true
}

/* ---------- 表单弹窗 ---------- */
const submitting = ref(false)
const onboardOpen = ref(false)
const offboardOpen = ref(false)
const promoteOpen = ref(false)
const onboardForm = reactive({ name: '', deptId: 'D02', position: '', entry_date: dateAdd(7), salary: 15000, managerId: '' })
const offboardForm = reactive({ employeeId: '', last_day: today(), reason: '', handover_to: '' })
const promoteForm = reactive({ employeeId: '', new_salary: 26000, new_position: '', effective_date: today(), reason: '' })

function openOnboard(): void {
  Object.assign(onboardForm, { name: '', deptId: departments.value[0]?.id || 'D01', position: '', entry_date: dateAdd(7), salary: 15000, managerId: '' })
  onboardOpen.value = true
}
function openOffboard(): void {
  Object.assign(offboardForm, { employeeId: employees.value[0]?.id || '', last_day: today(), reason: '', handover_to: '' })
  offboardOpen.value = true
}
function openPromote(): void {
  Object.assign(promoteForm, { employeeId: employees.value[0]?.id || '', new_salary: 26000, new_position: '', effective_date: today(), reason: '' })
  promoteOpen.value = true
}

async function submitOnboard(): Promise<void> {
  submitting.value = true
  try {
    const args = {
      ...aliased({
        name: onboardForm.name.trim() || 'New Employee',
        position: onboardForm.position.trim() || 'Employee',
        entry_date: onboardForm.entry_date || dateAdd(7),
        deptId: onboardForm.deptId,
        managerId: onboardForm.managerId || deptLeader(onboardForm.deptId),
      }),
      ...centsArgs('salary', onboardForm.salary),
    }
    const res = await submitTool('hr.onboard', args)
    if (res.ok) onboardOpen.value = false
  } finally {
    submitting.value = false
  }
}
async function submitOffboard(): Promise<void> {
  submitting.value = true
  try {
    const args = {
      ...aliased({
        employeeId: offboardForm.employeeId,
        last_day: offboardForm.last_day || today(),
        reason: offboardForm.reason.trim() || '—',
      }),
      handover_to: offboardForm.handover_to || '',
    }
    const res = await submitTool('hr.offboard', args)
    if (res.ok) offboardOpen.value = false
  } finally {
    submitting.value = false
  }
}
async function submitPromote(): Promise<void> {
  submitting.value = true
  try {
    const args = {
      ...aliased({
        employeeId: promoteForm.employeeId,
        new_position: promoteForm.new_position.trim(),
        effective_date: promoteForm.effective_date || today(),
        reason: promoteForm.reason.trim() || '—',
      }),
      ...centsArgs('new_salary', promoteForm.new_salary),
    }
    const res = await submitTool('hr.promote', args)
    if (res.ok) promoteOpen.value = false
  } finally {
    submitting.value = false
  }
}

/* ---------- 异动单审批 ---------- */
const approveOpen = ref(false)
const approveForm = reactive({ caseId: '', decision: 'approve', comment: '' })
function openApprove(row: HrCase): void {
  Object.assign(approveForm, { caseId: row.id, decision: 'approve', comment: '' })
  approveOpen.value = true
}
async function submitApprove(): Promise<void> {
  submitting.value = true
  try {
    const res = await submitTool('hr.approve', {
      case_id: approveForm.caseId,
      decision: approveForm.decision,
      comment: approveForm.comment.trim() || '—',
    })
    if (res.ok) approveOpen.value = false
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('hr.title') }}</h1><p>{{ t('hr.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button v-if="can('hr.onboard')" @click="openOnboard"><Icon name="plus" :size="14" /> {{ t('hr.newOnboard') }}</a-button>
        <a-button v-if="can('hr.offboard')" @click="openOffboard"><Icon name="user" :size="14" /> {{ t('hr.newOffboard') }}</a-button>
        <a-button v-if="can('hr.promote')" type="primary" @click="openPromote"><Icon name="trend" :size="14" /> {{ t('hr.newPromote') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="oa-grid g-4 mb16">
        <div class="stat-card" v-for="k in hrKpis" :key="k.label">
          <div class="stat-ico" :class="'bg-' + k.color"><Icon :name="k.ico" :size="18" /></div>
          <div class="stat-label">{{ k.label }}</div>
          <a-statistic :value="k.value" :value-style="{ fontSize: '23px', fontWeight: 700 }" />
        </div>
      </div>

      <a-card>
        <a-tabs v-model:activeKey="tab">
          <a-tab-pane key="roster" :tab="t('hr.tabRoster')">
            <div class="oa-toolbar">
              <a-select v-model:value="deptFilter" :options="[{ label: t('common.all') + t('common.dept'), value: '' }, ...deptOptions]" style="width: 160px" />
              <a-select v-model:value="statusFilter" :options="[{ label: t('common.all') + t('common.status'), value: '' }, ...statusOptions]" style="width: 140px" />
              <a-input v-model:value="keyword" :placeholder="t('hr.searchPh')" allow-clear style="width: 220px" />
              <span class="spacer"></span>
              <span class="small muted">{{ t('common.total') }} {{ filteredEmployees.length }} {{ t('common.result') }}</span>
            </div>
            <a-table class="oa-table" :columns="rosterColumns" :data-source="filteredEmployees" row-key="id" :pagination="{ pageSize: 8 }">
              <template #bodyCell="{ column, record }">
                <template v-if="column.key === 'dept'">{{ deptName(record.deptId) }}</template>
                <template v-else-if="column.key === 'status'">
                  <a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag>
                </template>
                <template v-else-if="column.key === 'salary'"><span class="cell-num">{{ money(record.salary) }}</span></template>
                <template v-else-if="column.key === 'manager'">{{ empName(record.managerId) }}</template>
                <template v-else-if="column.key === 'action'">
                  <a-button type="link" size="small" @click="openDetail('employee', record)">{{ t('common.view') }}</a-button>
                </template>
              </template>
            </a-table>
          </a-tab-pane>

          <a-tab-pane key="org" :tab="t('hr.tabOrg')">
            <div style="padding: 16px">
              <a-alert type="info" show-icon class="mb16" :message="t('hr.orgTitle')" :description="t('hr.orgDesc')" />
              <div class="org-root" v-if="departments.length">
                <div class="org-node">
                  <span class="org-ico"><Icon name="building" :size="16" /></span>
                  <div>
                    <div class="org-name">{{ departments[0].name }}</div>
                    <div class="cell-sub small">{{ t('hr.orgHead') }}: {{ empName(departments[0].leaderId) }}</div>
                  </div>
                  <a-tag color="blue" style="margin-left: auto">{{ employees.length }} {{ t('common.people') }}</a-tag>
                </div>
              </div>
              <div class="org-children">
                <div class="org-node" v-for="d in departments.slice(1)" :key="d.id">
                  <span class="org-ico"><Icon name="users" :size="16" /></span>
                  <div>
                    <div class="org-name">{{ d.name }}</div>
                    <div class="cell-sub small">{{ t('hr.orgHead') }}: {{ empName(d.leaderId) }}</div>
                  </div>
                  <a-tag color="green" style="margin-left: auto">{{ t('hr.orgCount') }} {{ headcount(d.id) }}</a-tag>
                </div>
              </div>
            </div>
          </a-tab-pane>

          <a-tab-pane key="cases" :tab="t('hr.tabCases')">
            <div class="oa-toolbar">
              <span class="small muted">{{ t('common.total') }} {{ cases.length }} {{ t('common.result') }}</span>
            </div>
            <a-table class="oa-table" :columns="caseColumns" :data-source="cases" row-key="id" :pagination="{ pageSize: 8 }">
              <template #bodyCell="{ column, record }">
                <template v-if="column.key === 'id'"><span class="cell-link" @click="openDetail('case', record)">{{ record.id }}</span></template>
                <template v-else-if="column.key === 'type'"><a-tag :color="tagColor(record.type)">{{ t('st.' + record.type) }}</a-tag></template>
                <template v-else-if="column.key === 'employee'">{{ empName(record.empId) }}</template>
                <template v-else-if="column.key === 'status'"><a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag></template>
                <template v-else-if="column.key === 'steps'">
                  <div class="small muted">{{ record.stepIndex }} / 5</div>
                  <a-progress :percent="Math.round(record.stepIndex / 5 * 100)" size="small" />
                </template>
                <template v-else-if="column.key === 'action'">
                  <a-button v-if="record.status === 'running' && can('hr.approve')" type="link" size="small" @click="openApprove(record)">{{ t('hr.approveAction') }}</a-button>
                  <a-button type="link" size="small" @click="openDetail('case', record)">{{ t('common.view') }}</a-button>
                </template>
              </template>
            </a-table>
          </a-tab-pane>
        </a-tabs>
      </a-card>
    </AsyncPanel>

    <!-- 详情抽屉 -->
    <a-drawer v-model:open="detailOpen" :title="detailTitle" :width="440">
      <div class="desc-grid">
        <div class="desc-item" v-for="(r, i) in detailRows" :key="i">
          <span class="desc-k">{{ r.k }}</span><span class="desc-v">{{ r.v }}</span>
        </div>
      </div>
    </a-drawer>

    <!-- 入职 -->
    <a-modal v-model:open="onboardOpen" :title="t('hr.onboardTitle')" :confirm-loading="submitting" @ok="submitOnboard">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · hr.onboard'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.name')"><a-input v-model:value="onboardForm.name" /></a-form-item>
          <a-form-item :label="t('field.dept')"><a-select v-model:value="onboardForm.deptId" :options="deptOptions" /></a-form-item>
          <a-form-item :label="t('field.position')"><a-input v-model:value="onboardForm.position" /></a-form-item>
          <a-form-item :label="t('field.entryDate')"><a-date-picker v-model:value="onboardForm.entry_date" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.salary')"><a-input-number v-model:value="onboardForm.salary" :min="0" :step="1000" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.manager')"><a-select v-model:value="onboardForm.managerId" :options="empOptions" allow-clear /></a-form-item>
        </div>
      </a-form>
    </a-modal>

    <!-- 离职（敏感，需二次确认） -->
    <a-modal v-model:open="offboardOpen" :title="t('hr.offboardTitle')" :confirm-loading="submitting" @ok="submitOffboard">
      <a-alert type="warning" show-icon class="form-hint" :message="t('common.byAgent') + ' · hr.offboard'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')"><a-select v-model:value="offboardForm.employeeId" :options="empOptions" /></a-form-item>
          <a-form-item :label="t('field.lastDay')"><a-date-picker v-model:value="offboardForm.last_day" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.reason')"><a-input v-model:value="offboardForm.reason" /></a-form-item>
          <a-form-item :label="t('field.handover')"><a-select v-model:value="offboardForm.handover_to" :options="empOptions" allow-clear /></a-form-item>
        </div>
      </a-form>
    </a-modal>

    <!-- 晋升 / 涨薪（敏感，需二次确认） -->
    <a-modal v-model:open="promoteOpen" :title="t('hr.promoteTitle')" :confirm-loading="submitting" @ok="submitPromote">
      <a-alert type="warning" show-icon class="form-hint" :message="t('common.byAgent') + ' · hr.promote'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.employee')"><a-select v-model:value="promoteForm.employeeId" :options="empOptions" /></a-form-item>
          <a-form-item :label="t('field.newSalary')"><a-input-number v-model:value="promoteForm.new_salary" :min="0" :step="1000" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.newPosition')"><a-input v-model:value="promoteForm.new_position" /></a-form-item>
          <a-form-item :label="t('field.effectiveDate')"><a-date-picker v-model:value="promoteForm.effective_date" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.reason')"><a-input v-model:value="promoteForm.reason" /></a-form-item>
        </div>
      </a-form>
    </a-modal>

    <!-- 异动单审批（敏感，逐级推进） -->
    <a-modal v-model:open="approveOpen" :title="t('hr.approveTitle')" :confirm-loading="submitting" @ok="submitApprove">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · hr.approve'" />
      <a-form layout="vertical">
        <a-form-item :label="t('field.decision')">
          <a-radio-group v-model:value="approveForm.decision">
            <a-radio-button value="approve">{{ t('hr.approve') }}</a-radio-button>
            <a-radio-button value="reject">{{ t('hr.reject') }}</a-radio-button>
          </a-radio-group>
        </a-form-item>
        <a-form-item :label="t('field.comment')">
          <a-textarea v-model:value="approveForm.comment" :rows="3" :placeholder="t('hr.commentPh')" />
        </a-form-item>
      </a-form>
    </a-modal>
  </div>
</template>
