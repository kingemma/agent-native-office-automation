<script setup lang="ts">
/** 合同：台账 / 状态·关键词筛选 / 到期预警 / 新建（敏感确认）/ 详情抽屉 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import AttachmentUploader from '../components/AttachmentUploader.vue'
import { listTable } from '../api/bridge'
import {
  CONTRACT_TYPES, toContract, toEmployee, toFlow,
  type ApprovalFlow, type Contract, type Employee,
} from '../model'
import { aliased, centsArgs, dateAdd, money, tagColor, today, wan } from '../utils'
import { auth, CURRENT_ACTOR, can, dataVersion, submitTool } from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const contracts = ref<Contract[]>([])
const employees = ref<Employee[]>([])
const flows = ref<ApprovalFlow[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [co, em, fl] = await Promise.all([
      listTable('contracts'),
      listTable('employees'),
      listTable('approval_flows'),
    ])
    contracts.value = co.map(toContract)
    employees.value = em.map(toEmployee)
    flows.value = fl.map(toFlow)
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
watch(dataVersion, load)

const typeFilter = ref('all')
const statusFilter = ref('')
const keyword = ref('')
const typeOptions = computed(() => [
  { label: t('common.all'), value: 'all' },
  ...CONTRACT_TYPES.map((k) => ({ label: t('st.' + k), value: k })),
])
const statusOptions = computed(() => [
  { label: t('common.all') + t('common.status'), value: '' },
  ...['effective', 'expiring', 'review', 'archived'].map((k) => ({ label: t('st.' + k), value: k })),
])
function empName(id: string): string {
  return employees.value.find((e) => e.id === id)?.name || id || '—'
}
function ownerName(id: string): string {
  return id ? empName(id) : '—'
}
const filteredContracts = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return contracts.value.filter((c) => {
    if (typeFilter.value !== 'all' && c.type !== typeFilter.value) return false
    if (statusFilter.value && c.status !== statusFilter.value) return false
    if (!kw) return true
    return `${c.id} ${c.name} ${c.party}`.toLowerCase().includes(kw)
  })
})
const expiringContracts = computed(() =>
  contracts.value.filter(
    (c) => c.status !== 'archived' &&
      (c.status === 'expiring' || (c.expireDate !== '—' && c.expireDate <= dateAdd(30) && c.expireDate >= today())),
  ),
)
const contractKpis = computed(() => [
  { label: t('contract.kpiEffective'), value: String(contracts.value.filter((c) => c.status === 'effective').length), ico: 'file', color: 'green' },
  { label: t('contract.kpiAmount'), value: wan(contracts.value.reduce((n, c) => n + c.amount, 0)), ico: 'wallet', color: 'blue' },
  { label: t('contract.kpiExpiring'), value: String(expiringContracts.value.length), ico: 'alert', color: 'orange' },
  { label: t('contract.kpiThisYear'), value: String(contracts.value.filter((c) => c.signDate.indexOf(String(new Date().getFullYear())) === 0).length), ico: 'trend', color: 'purple' },
])
const columns = computed(() => [
  { title: t('contract.colNo'), dataIndex: 'id', key: 'id' },
  { title: t('contract.colName'), dataIndex: 'name', key: 'name' },
  { title: t('contract.colParty'), dataIndex: 'party', key: 'party' },
  { title: t('contract.colType'), key: 'type' },
  { title: t('contract.colAmount'), key: 'amount', align: 'right' as const },
  { title: t('contract.colSign'), dataIndex: 'signDate', key: 'sign' },
  { title: t('contract.colExpire'), dataIndex: 'expireDate', key: 'expire' },
  { title: t('contract.colStatus'), key: 'status' },
  { title: t('contract.colOwner'), key: 'owner' },
  { title: t('common.action'), key: 'action' },
])

/* ---------- 多级审批（法务 → 用印，只有当前节点审批人可操作） ---------- */
function flowsOf(id: string): ApprovalFlow[] {
  return flows.value.filter((f) => f.entityId === id).sort((a, b) => a.step - b.step)
}
/** 当前待审节点 = step 最小的 pending 节点，与后端逐级推进的判定保持一致 */
function currentNode(id: string): ApprovalFlow | undefined {
  return flowsOf(id).find((f) => f.status === 'pending')
}
function isMyTurn(c: Contract): boolean {
  if (c.status !== 'review' || !can('contract.approve')) return false
  if (auth.user?.role === 'admin') return true
  return currentNode(c.id)?.approver === auth.user?.employee_id
}

const approveOpen = ref(false)
const approveTarget = ref<Contract | null>(null)
const approveDecision = ref<'approve' | 'reject'>('approve')
const approveComment = ref('')
const approving = ref(false)

function openApprove(c: Contract, decision: 'approve' | 'reject'): void {
  approveTarget.value = c
  approveDecision.value = decision
  approveComment.value = ''
  approveOpen.value = true
}

async function submitApprove(): Promise<void> {
  const c = approveTarget.value
  if (!c) return
  approving.value = true
  try {
    // 弹窗本身就是确认环节，带 confirmed 跳过 submitTool 的二次确认
    const res = await submitTool(
      'contract.approve',
      { contract_id: c.id, decision: approveDecision.value, comment: approveComment.value.trim(), confirmed: true },
      CURRENT_ACTOR,
    )
    if (res.ok) approveOpen.value = false
  } finally {
    approving.value = false
  }
}

/* ---------- 详情抽屉 ---------- */
const detailOpen = ref(false)
const detailRow = ref<Contract | null>(null)
const detailRows = computed<{ k: string; v: string }[]>(() => {
  const c = detailRow.value
  if (!c) return []
  return [
    { k: t('contract.colNo'), v: c.id }, { k: t('contract.colName'), v: c.name },
    { k: t('contract.colParty'), v: c.party }, { k: t('contract.colType'), v: t('st.' + c.type) },
    { k: t('contract.colAmount'), v: money(c.amount) }, { k: t('contract.colSign'), v: c.signDate },
    { k: t('contract.colExpire'), v: c.expireDate }, { k: t('contract.colStatus'), v: t('st.' + c.status) },
    { k: t('contract.colOwner'), v: ownerName(c.ownerId) },
  ]
})
function openDetail(row: Contract): void {
  detailRow.value = row
  detailOpen.value = true
}
const detailFlows = computed<ApprovalFlow[]>(() => (detailRow.value ? flowsOf(detailRow.value.id) : []))

/* ---------- 新建合同（敏感，需二次确认） ---------- */
const formOpen = ref(false)
const submitting = ref(false)
const form = reactive({ name: '', party: '', type: 'service', amount: 100000, sign_date: today(), expire_date: dateAdd(365) })
function openForm(): void {
  Object.assign(form, { name: '', party: '', type: 'service', amount: 100000, sign_date: today(), expire_date: dateAdd(365) })
  formOpen.value = true
}
async function submitForm(): Promise<void> {
  submitting.value = true
  try {
    const args = {
      ...aliased({
        name: form.name.trim() || '—',
        party: form.party.trim() || '—',
        type: form.type,
        sign_date: form.sign_date || today(),
        expire_date: form.expire_date || dateAdd(365),
        // 负责人取当前登录人：审批链会把他排除在外，避免负责人自审
        ownerId: auth.user?.employee_id || '',
      }),
      ...centsArgs('amount', form.amount),
    }
    const res = await submitTool('contract.create', args, CURRENT_ACTOR)
    if (res.ok) formOpen.value = false
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('contract.title') }}</h1><p>{{ t('contract.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button v-if="can('contract.create')" type="primary" @click="openForm"><Icon name="plus" :size="14" /> {{ t('contract.newContract') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="oa-grid g-4 mb16">
        <div class="stat-card" v-for="k in contractKpis" :key="k.label">
          <div class="stat-ico" :class="'bg-' + k.color"><Icon :name="k.ico" :size="18" /></div>
          <div class="stat-label">{{ k.label }}</div>
          <a-statistic :value="k.value" :value-style="{ fontSize: '23px', fontWeight: 700 }" />
        </div>
      </div>

      <a-alert v-if="expiringContracts.length" type="warning" show-icon class="mb16" :message="t('contract.warnTitle')">
        <template #description>
          {{ t('contract.warnDesc') }}
          <code class="tool-name" v-for="c in expiringContracts" :key="c.id" style="margin-left: 6px">
            {{ c.id }} {{ c.expireDate }}
          </code>
        </template>
      </a-alert>

      <a-card>
        <div class="oa-toolbar">
          <a-select v-model:value="typeFilter" :options="typeOptions" style="width: 150px" />
          <a-select v-model:value="statusFilter" :options="statusOptions" style="width: 150px" />
          <a-input v-model:value="keyword" :placeholder="t('contract.searchPh')" allow-clear style="width: 230px" />
          <span class="spacer"></span>
          <span class="small muted">{{ t('common.total') }} {{ filteredContracts.length }} {{ t('common.result') }}</span>
        </div>
        <a-table class="oa-table" :columns="columns" :data-source="filteredContracts" row-key="id" :pagination="{ pageSize: 8 }">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'id'"><span class="cell-link" @click="openDetail(record)">{{ record.id }}</span></template>
            <template v-else-if="column.key === 'type'"><a-tag :color="tagColor(record.type)">{{ t('st.' + record.type) }}</a-tag></template>
            <template v-else-if="column.key === 'amount'"><span class="cell-num">{{ money(record.amount) }}</span></template>
            <template v-else-if="column.key === 'expire'">
              <span :style="{ color: record.status === 'expiring' ? '#d48806' : '' }" class="mono">{{ record.expireDate }}</span>
            </template>
            <template v-else-if="column.key === 'status'"><a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag></template>
            <template v-else-if="column.key === 'owner'">{{ ownerName(record.ownerId) }}</template>
            <template v-else-if="column.key === 'action'">
              <div class="row-actions">
                <a-button v-if="isMyTurn(record)" size="small" type="primary" ghost @click="openApprove(record, 'approve')">{{ t('contract.approve') }}</a-button>
                <a-button v-if="isMyTurn(record)" size="small" danger @click="openApprove(record, 'reject')">{{ t('contract.reject') }}</a-button>
                <a-button type="link" size="small" @click="openDetail(record)">{{ t('common.view') }}</a-button>
              </div>
            </template>
          </template>
        </a-table>
      </a-card>
    </AsyncPanel>

    <!-- 详情抽屉 -->
    <a-drawer v-model:open="detailOpen" :title="t('contract.detailTitle')" :width="440">
      <div class="desc-grid">
        <div class="desc-item" v-for="(r, i) in detailRows" :key="i">
          <span class="desc-k">{{ r.k }}</span><span class="desc-v">{{ r.v }}</span>
        </div>
      </div>

      <!-- 审批进度：法务 → 用印，逐级流转留痕 -->
      <div class="flow-box">
        <h4>{{ t('contract.flowTitle') }}</h4>
        <div v-if="!detailFlows.length" class="small muted">{{ t('contract.flowEmpty') }}</div>
        <div class="flow-row" v-for="f in detailFlows" :key="f.id">
          <span class="flow-step" :class="'is-' + f.status">{{ f.step }}</span>
          <div class="flow-main">
            <div class="flow-line">
              <b>{{ empName(f.approver) }}</b>
              <a-tag :color="f.status === 'done' ? 'green' : f.status === 'rejected' ? 'red' : 'blue'">
                {{ f.status === 'done' ? t('st.approved') : f.status === 'rejected' ? t('st.rejected') : t('contract.waiting') }}
              </a-tag>
            </div>
            <div v-if="f.comment && f.comment !== '—'" class="small muted">{{ f.comment }}</div>
            <div v-if="f.actedAt && f.actedAt !== '—'" class="small muted">{{ f.actedAt }}</div>
          </div>
        </div>
      </div>

      <!-- 合同扫描件 / 补充协议：走统一上传接口 -->
      <AttachmentUploader v-if="detailRow" entity-type="contract" :entity-id="detailRow.id" />
    </a-drawer>

    <!-- 合同审批（弹窗本身即确认） -->
    <a-modal v-model:open="approveOpen" :title="t('contract.approveTitle')" :confirm-loading="approving" @ok="submitApprove">
      <div class="small muted mb8" v-if="approveTarget">
        {{ approveTarget.id }} · {{ approveTarget.name }} · {{ money(approveTarget.amount) }}
      </div>
      <a-alert
        v-if="approveTarget"
        :type="approveDecision === 'reject' ? 'warning' : 'info'"
        show-icon
        class="form-hint"
        :message="t('contract.stepLabel', { n: currentNode(approveTarget.id)?.step || 1 }) + ' · ' + t('contract.myTurn')"
      />
      <a-textarea v-model:value="approveComment" :placeholder="t('contract.commentPh')" :rows="3" />
    </a-modal>

    <!-- 新建合同 -->
    <a-modal v-model:open="formOpen" :title="t('contract.newContract')" :confirm-loading="submitting" @ok="submitForm">
      <a-alert type="warning" show-icon class="form-hint" :message="t('common.byAgent') + ' · contract.create'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.contractName')"><a-input v-model:value="form.name" /></a-form-item>
          <a-form-item :label="t('field.party')"><a-input v-model:value="form.party" /></a-form-item>
          <a-form-item :label="t('field.contractType')"><a-select v-model:value="form.type" :options="CONTRACT_TYPES.map(k => ({ label: t('st.' + k), value: k }))" /></a-form-item>
          <a-form-item :label="t('field.amount')"><a-input-number v-model:value="form.amount" :min="0" :step="10000" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.signDate')"><a-date-picker v-model:value="form.sign_date" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.expireDate')"><a-date-picker v-model:value="form.expire_date" value-format="YYYY-MM-DD" style="width: 100%" /></a-form-item>
        </div>
      </a-form>
    </a-modal>
  </div>
</template>
