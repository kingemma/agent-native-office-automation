<script setup lang="ts">
/** 财务：三类报销 / 审批 / 新建报销 / 预算执行 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import AttachmentUploader from '../components/AttachmentUploader.vue'
import { getMetrics, listTable, uploadAttachment, type Metrics } from '../api/bridge'
import {
  CLAIM_ITEM_STANDARDS, CLAIM_TYPES, toClaim, toClaimItem, toDepartment, toEmployee, toFlow,
  type ApprovalFlow, type Claim, type ClaimItem, type Department, type Employee,
} from '../model'
import { aliased, centsArgs, money, tagColor, wan } from '../utils'
import { auth, CURRENT_ACTOR, can, dataVersion, submitTool } from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const claims = ref<Claim[]>([])
const claimItems = ref<ClaimItem[]>([])
const employees = ref<Employee[]>([])
const departments = ref<Department[]>([])
const metrics = ref<Metrics | null>(null)

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [cl, em, de, me, fl, ci] = await Promise.all([
      listTable('claims'),
      listTable('employees'),
      listTable('departments'),
      getMetrics(),
      listTable('approval_flows'),
      listTable('claim_items'),
    ])
    claims.value = cl.map(toClaim)
    employees.value = em.map(toEmployee)
    departments.value = de.map(toDepartment)
    metrics.value = me
    flows.value = fl.map(toFlow)
    claimItems.value = ci.map(toClaimItem)
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
watch(dataVersion, load)

const tab = ref<'all' | 'daily' | 'business' | 'project' | 'budget'>('all')
const keyword = ref('')
const empOptions = computed(() =>
  employees.value
    .filter((e) => e.status !== 'left')
    .filter((e) => auth.user?.role !== 'employee' || e.id === auth.user?.employee_id)
    .map((e) => ({ label: `${e.name} · ${e.no}`, value: e.id })),
)
const typeOptions = computed(() => CLAIM_TYPES.map((k) => ({ label: t('st.' + k), value: k })))

function empName(id: string): string {
  return employees.value.find((e) => e.id === id)?.name || id || '—'
}
function deptName(id: string): string {
  return departments.value.find((d) => d.id === id)?.name || '—'
}
function applicantDept(id: string): string {
  return employees.value.find((e) => e.id === id)?.deptId || 'D01'
}

const filteredClaims = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return claims.value.filter((c) => {
    if (tab.value !== 'all' && tab.value !== 'budget' && c.type !== tab.value) return false
    if (!kw) return true
    return `${c.id} ${c.purpose} ${c.project} ${empName(c.applicantId)}`.toLowerCase().includes(kw)
  })
})
const totalAmount = computed(() => claims.value.filter((c) => c.status !== 'rejected').reduce((n, c) => n + c.amount, 0))
const paidAmount = computed(() => claims.value.filter((c) => c.status === 'paid').reduce((n, c) => n + c.amount, 0))
const projectAmount = computed(() => claims.value.filter((c) => c.type === 'project').reduce((n, c) => n + c.amount, 0))
const financeKpis = computed(() => [
  { label: t('finance.kpiMonth'), value: wan(totalAmount.value), ico: 'wallet', color: 'blue' },
  { label: t('finance.kpiPending'), value: String(claims.value.filter((c) => c.status === 'pending').length), ico: 'inbox', color: 'orange' },
  { label: t('finance.kpiPaid'), value: wan(paidAmount.value), ico: 'checked', color: 'green' },
  { label: t('finance.kpiProject'), value: (totalAmount.value ? Math.round((projectAmount.value / totalAmount.value) * 100) : 0) + '%', ico: 'pie', color: 'purple' },
])
const budgetUsed = computed(() => Number(metrics.value?.budget_used_cents || 0) / 100)
const budgetTotal = computed(() => Number(metrics.value?.budget_total_cents || 0) / 100)
const budgetPct = computed(() => budgetTotal.value ? Math.min(100, Math.round((budgetUsed.value / budgetTotal.value) * 100)) : 0)
const typeRows = computed(() => CLAIM_TYPES.map((k, i) => ({
  key: k,
  color: ['#1677ff', '#722ed1', '#13c2c2'][i],
  value: claims.value.filter((c) => c.type === k && c.status !== 'rejected').reduce((n, c) => n + c.amount, 0),
})))

const columns = computed(() => [
  { title: t('finance.colNo'), dataIndex: 'id', key: 'id' },
  { title: t('finance.colType'), key: 'type' },
  { title: t('finance.colApplicant'), key: 'applicant' },
  { title: t('finance.colDept'), key: 'dept' },
  { title: t('finance.colPurpose'), dataIndex: 'purpose', key: 'purpose' },
  { title: t('finance.colProject'), dataIndex: 'project', key: 'project' },
  { title: t('finance.colAmount'), key: 'amount', align: 'right' as const },
  { title: t('finance.colStatus'), key: 'status' },
  { title: t('finance.colSubmit'), dataIndex: 'submitDate', key: 'submit' },
  { title: t('common.action'), key: 'action' },
])

/* ---------- 审批（多级流转：只有当前节点审批人能操作） ---------- */
const flows = ref<ApprovalFlow[]>([])
function flowsOf(id: string): ApprovalFlow[] {
  return flows.value.filter((f) => f.entityId === id).sort((a, b) => a.step - b.step)
}
/** 当前待审节点 = step 最小的 pending 节点，与后端逐级推进的判定保持一致 */
function currentNode(id: string): ApprovalFlow | undefined {
  return flowsOf(id).find((f) => f.status === 'pending')
}
function isMyTurn(c: Claim): boolean {
  if (c.status !== 'pending' || !can('finance.claim.approve')) return false
  if (auth.user?.role === 'admin') return true
  return currentNode(c.id)?.approver === auth.user?.employee_id
}

const approveOpen = ref(false)
const approveTarget = ref<Claim | null>(null)
const approveDecision = ref<'approve' | 'reject'>('approve')
const approveComment = ref('')
const approving = ref(false)

function openApprove(c: Claim, decision: 'approve' | 'reject'): void {
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
      'finance.claim.approve',
      { claim_id: c.id, decision: approveDecision.value, comment: approveComment.value.trim(), confirmed: true },
      CURRENT_ACTOR,
    )
    if (res.ok) approveOpen.value = false
  } finally {
    approving.value = false
  }
}

/* ---------- 详情抽屉 ---------- */
const detailOpen = ref(false)
const detailRow = ref<Claim | null>(null)
const detailRows = computed<{ k: string; v: string }[]>(() => {
  const c = detailRow.value
  if (!c) return []
  return [
    { k: t('finance.colNo'), v: c.id }, { k: t('finance.colType'), v: t('st.' + c.type) },
    { k: t('finance.colApplicant'), v: empName(c.applicantId) }, { k: t('finance.colDept'), v: deptName(c.deptId) },
    { k: t('finance.colAmount'), v: money(c.amount) }, { k: t('finance.colPurpose'), v: c.purpose },
    { k: t('finance.colProject'), v: c.project }, { k: t('finance.colStatus'), v: t('st.' + c.status) },
    { k: t('finance.colSubmit'), v: c.submitDate }, { k: t('finance.colApprover'), v: empName(c.approverId) },
  ]
})
function openDetail(row: Claim): void {
  detailRow.value = row
  detailOpen.value = true
}
/**
 * 附件编辑权只给申请人本人：审批人进详情是为了核对发票，不该在上游单据上增删材料。
 * 与「填写时上传、审批时不上传」的流程要求保持一致。
 */
function canEditAttachments(c: Claim): boolean {
  return c.applicantId === auth.user?.employee_id
}
const detailFlows = computed<ApprovalFlow[]>(() => (detailRow.value ? flowsOf(detailRow.value.id) : []))
const detailItems = computed<ClaimItem[]>(() =>
  detailRow.value ? claimItems.value.filter((i) => i.claimId === detailRow.value!.id) : [],
)
const itemColumns = computed(() => [
  { title: t('finance.itemCategory'), key: 'category' },
  { title: t('finance.itemDate'), dataIndex: 'occurredOn', key: 'date' },
  { title: t('finance.itemInvoice'), dataIndex: 'invoiceNo', key: 'invoice' },
  { title: t('finance.itemStandard'), key: 'standard', align: 'right' as const },
  { title: t('field.amount'), key: 'amount', align: 'right' as const },
])

/* ---------- 新建报销 ---------- */
interface ItemDraft {
  category: string
  amount: number
  occurredOn: string
  invoiceNo: string
  note: string
}

const ITEM_CATEGORIES = Object.keys(CLAIM_ITEM_STANDARDS)
const categoryOptions = computed(() => ITEM_CATEGORIES.map((k) => ({ label: t('st.' + k), value: k })))

const claimOpen = ref(false)
const submitting = ref(false)
const claimForm = reactive({
  type: 'daily', applicantId: '', amount: 1000, purpose: '', project: '',
  items: [] as ItemDraft[],
})
const itemsTotal = computed(() =>
  claimForm.items.reduce((n, it) => n + (Number(it.amount) || 0), 0),
)

/** 某类别的标准（分）；未知类别返回 0（不校验） */
function standardOf(category: string): number {
  return CLAIM_ITEM_STANDARDS[category]?.cents || 0
}
/** 与后端同一口径：金额换算成分后再与标准比较 */
function isOverStandard(item: ItemDraft): boolean {
  const cap = standardOf(item.category)
  return cap > 0 && Math.round((Number(item.amount) || 0) * 100) > cap
}
function standardLabel(category: string): string {
  const std = CLAIM_ITEM_STANDARDS[category]
  if (!std) return '—'
  return t('finance.standardLine', { cap: std.cents / 100, unit: t('finance.stdUnit.' + std.unit) })
}
const standardTip = computed(() =>
  ITEM_CATEGORIES.map((k) => `${t('st.' + k)} ${standardLabel(k)}`).join(' · '),
)
/** 超标但没写说明的条数：>0 时禁止提交（后端同样会拦） */
const missingNotes = computed(
  () => claimForm.items.filter((it) => isOverStandard(it) && !it.note.trim()).length,
)

/**
 * 填报阶段的附件暂存：附件表要挂 claim_id，而单号要等创建接口返回，
 * 所以先在前端存住 File，创建成功后再逐张上传（见 submitClaim）。
 */
const stagedFiles = ref<File[]>([])
const fileInput = ref<HTMLInputElement | null>(null)

function pickFiles(): void {
  fileInput.value?.click()
}
function onStagedFiles(event: Event): void {
  const input = event.target as HTMLInputElement
  stagedFiles.value = [...stagedFiles.value, ...Array.from(input.files || [])]
  input.value = ''
}
function removeStaged(index: number): void {
  stagedFiles.value.splice(index, 1)
}
function sizeText(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}
/** 单张失败不打断其余上传，最后把失败文件名列出来让用户去详情里补传 */
async function uploadStaged(claimId: string): Promise<void> {
  const failed: string[] = []
  for (const file of stagedFiles.value) {
    try {
      await uploadAttachment(file, 'claim', claimId)
    } catch (e) {
      // 单已建好，附件失败不该回滚整单：记录后提示用户重传
      console.error(`附件上传失败：${file.name}`, e)
      failed.push(file.name)
    }
  }
  if (failed.length) {
    message.error(t('attach.uploadFailed', { files: failed.join('、') }))
  } else {
    message.success(t('attach.uploaded'))
  }
}

function openClaim(): void {
  // 默认给当前登录人：普通员工的下拉里只有自己，取 employees[0] 会落到 E01 上被「不能代他人」拒绝
  const me = auth.user?.employee_id || ''
  const options = empOptions.value
  const applicantId = options.some((o) => o.value === me) ? me : (options[0]?.value || '')
  Object.assign(claimForm, {
    type: 'daily', applicantId, amount: 1000, purpose: '', project: '', items: [],
  })
  stagedFiles.value = []
  claimOpen.value = true
}
function addItem(): void {
  claimForm.items.push({ category: 'transport', amount: 0, occurredOn: '', invoiceNo: '', note: '' })
}
function removeItem(index: number): void {
  claimForm.items.splice(index, 1)
}

async function submitClaim(): Promise<void> {
  // 超标必须写说明：前端先挡一层，后端 parse_claim_items 仍会二次校验
  if (missingNotes.value > 0) {
    message.warning(t('finance.overNeedNote', { n: missingNotes.value }))
    return
  }
  submitting.value = true
  try {
    // 有明细时金额由明细合计决定，与后端「明细优先」的规则保持一致
    const amount = claimForm.items.length ? itemsTotal.value : claimForm.amount
    const args = {
      ...aliased({
        type: claimForm.type,
        applicantId: claimForm.applicantId,
        deptId: applicantDept(claimForm.applicantId),
        purpose: claimForm.purpose.trim() || '—',
      }),
      project: claimForm.project.trim() || null,
      ...centsArgs('amount', amount),
      items: claimForm.items.length
        ? claimForm.items.map((it) => ({
            category: it.category,
            amount: Number(it.amount) || 0,
            occurred_on: it.occurredOn.trim(),
            invoice_no: it.invoiceNo.trim(),
            note: it.note.trim(),
          }))
        : undefined,
    }
    const res = await submitTool('finance.claim.create', args, CURRENT_ACTOR)
    if (!res.ok) return
    // 详情抽屉的附件列表挂在单号上，所以只能先建单、拿到 claim_id 再传
    const createdId = typeof res.claim_id === 'string' ? res.claim_id : ''
    if (createdId && stagedFiles.value.length) await uploadStaged(createdId)
    stagedFiles.value = []
    claimOpen.value = false
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('finance.title') }}</h1><p>{{ t('finance.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button type="primary" @click="openClaim"><Icon name="plus" :size="14" /> {{ t('finance.newClaim') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="oa-grid g-4 mb16">
        <div class="stat-card" v-for="k in financeKpis" :key="k.label">
          <div class="stat-ico" :class="'bg-' + k.color"><Icon :name="k.ico" :size="18" /></div>
          <div class="stat-label">{{ k.label }}</div>
          <a-statistic :value="k.value" :value-style="{ fontSize: '23px', fontWeight: 700 }" />
        </div>
      </div>

      <a-card>
        <a-tabs v-model:activeKey="tab">
          <a-tab-pane key="all" :tab="t('finance.tabAll')" />
          <a-tab-pane v-for="ty in CLAIM_TYPES" :key="ty" :tab="t('st.' + ty)" />
          <a-tab-pane key="budget" :tab="t('finance.budgetTitle')" />
        </a-tabs>

        <div class="oa-toolbar">
          <a-input v-model:value="keyword" :placeholder="t('finance.searchPh')" allow-clear style="width: 230px" />
          <span class="spacer"></span>
          <span class="small muted">{{ t('common.total') }} {{ filteredClaims.length }} {{ t('common.result') }}</span>
        </div>

        <!-- 报销表格 -->
        <div v-if="tab !== 'budget'">
          <a-table class="oa-table" :columns="columns" :data-source="filteredClaims" row-key="id" :pagination="{ pageSize: 8 }">
            <template #bodyCell="{ column, record }">
              <template v-if="column.key === 'id'"><span class="cell-link" @click="openDetail(record)">{{ record.id }}</span></template>
              <template v-else-if="column.key === 'type'"><a-tag :color="tagColor(record.type)">{{ t('st.' + record.type) }}</a-tag></template>
              <template v-else-if="column.key === 'applicant'">{{ empName(record.applicantId) }}</template>
              <template v-else-if="column.key === 'dept'">{{ deptName(record.deptId) }}</template>
              <template v-else-if="column.key === 'amount'"><span class="cell-num">{{ money(record.amount) }}</span></template>
              <template v-else-if="column.key === 'status'"><a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag></template>
              <template v-else-if="column.key === 'action'">
                <div class="row-actions">
                  <a-button v-if="isMyTurn(record)" size="small" type="primary" ghost @click="openApprove(record, 'approve')">{{ t('finance.approve') }}</a-button>
                  <a-button v-if="isMyTurn(record)" size="small" danger @click="openApprove(record, 'reject')">{{ t('finance.reject') }}</a-button>
                  <a-button type="link" size="small" @click="openDetail(record)">{{ t('common.view') }}</a-button>
                </div>
              </template>
            </template>
          </a-table>
        </div>

        <!-- 预算执行 -->
        <div v-else style="padding: 8px 16px 20px">
          <div class="oa-grid g-2">
            <div>
              <h3 style="margin-top: 0">{{ t('finance.budgetTitle') }}</h3>
              <p class="small muted">{{ t('finance.budgetDesc') }}</p>
              <div style="display: flex; justify-content: space-between; margin: 12px 0">
                <span class="small">{{ t('finance.used') }} <b>{{ money(budgetUsed) }}</b></span>
                <span class="small">{{ t('finance.remain') }} <b>{{ money(budgetTotal - budgetUsed) }}</b></span>
              </div>
              <a-progress :percent="budgetPct" :stroke-width="14" />
              <div class="small muted" style="margin-top: 10px">
                {{ budgetPct }}% · {{ t('finance.budgetRate') }} · {{ money(budgetTotal) }}
              </div>
            </div>
            <div>
              <h3 style="margin-top: 0">{{ t('dashboard.typeSplit') }}</h3>
              <div v-for="r in typeRows" :key="r.key" style="margin-bottom: 14px">
                <div style="display: flex; justify-content: space-between; font-size: 12.5px; margin-bottom: 5px">
                  <span>{{ t('st.' + r.key) }}</span><b>{{ money(r.value) }}</b>
                </div>
                <a-progress :percent="totalAmount ? Math.round(r.value / totalAmount * 100) : 0" :stroke-color="r.color" />
              </div>
            </div>
          </div>
        </div>
      </a-card>
    </AsyncPanel>

    <!-- 详情抽屉 -->
    <a-drawer v-model:open="detailOpen" :title="t('finance.detailTitle')" :width="440">
      <div class="desc-grid">
        <div class="desc-item" v-for="(r, i) in detailRows" :key="i">
          <span class="desc-k">{{ r.k }}</span><span class="desc-v">{{ r.v }}</span>
        </div>
      </div>

      <!-- 费用明细 -->
      <div class="flow-box" v-if="detailItems.length">
        <h4>{{ t('finance.itemsTitle') }}</h4>
        <a-table :columns="itemColumns" :data-source="detailItems" row-key="id" size="small" :pagination="false">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'category'">{{ t('st.' + record.category) }}</template>
            <template v-else-if="column.key === 'standard'">
              <span class="cell-num">{{ standardLabel(record.category) }}</span>
              <a-tag v-if="record.overStandard" color="orange" style="margin-left: 6px">
                {{ t('finance.overStandard') }}
              </a-tag>
            </template>
            <template v-else-if="column.key === 'amount'"><span class="cell-num">{{ money(record.amount) }}</span></template>
          </template>
        </a-table>
      </div>

      <!-- 审批进度：逐级流转留痕 -->
      <div class="flow-box">
        <h4>{{ t('finance.flowTitle') }}</h4>
        <div v-if="!detailFlows.length" class="small muted">{{ t('finance.flowEmpty') }}</div>
        <div class="flow-row" v-for="f in detailFlows" :key="f.id">
          <span class="flow-step" :class="'is-' + f.status">{{ f.step }}</span>
          <div class="flow-main">
            <div class="flow-line">
              <b>{{ empName(f.approver) }}</b>
              <a-tag :color="f.status === 'done' ? 'green' : f.status === 'rejected' ? 'red' : 'blue'">
                {{ f.status === 'done' ? t('st.approved') : f.status === 'rejected' ? t('st.rejected') : t('finance.waiting') }}
              </a-tag>
            </div>
            <div v-if="f.comment && f.comment !== '—'" class="small muted">{{ f.comment }}</div>
            <div v-if="f.actedAt && f.actedAt !== '—'" class="small muted">{{ f.actedAt }}</div>
          </div>
        </div>
      </div>

      <!-- 发票 / 附件：申请人本人可增删，审批人只读，避免审批环节改动上游单据 -->
      <AttachmentUploader
        v-if="detailRow"
        entity-type="claim"
        :entity-id="detailRow.id"
        :readonly="!canEditAttachments(detailRow)"
      />
    </a-drawer>

    <!-- 审批（收集意见，弹窗本身即确认） -->
    <a-modal v-model:open="approveOpen" :title="t('finance.approveTitle')" :confirm-loading="approving" @ok="submitApprove">
      <div class="small muted mb8" v-if="approveTarget">
        {{ approveTarget.id }} · {{ empName(approveTarget.applicantId) }} · {{ money(approveTarget.amount) }}
      </div>
      <a-alert
        v-if="approveTarget"
        :type="approveDecision === 'reject' ? 'warning' : 'info'"
        show-icon
        class="form-hint"
        :message="t('finance.stepLabel', { n: currentNode(approveTarget.id)?.step || 1 }) + ' · ' + t('finance.myTurn')"
      />
      <a-textarea v-model:value="approveComment" :placeholder="t('finance.commentPh')" :rows="3" />
    </a-modal>

    <!-- 新建报销 -->
    <a-modal
      v-model:open="claimOpen"
      :title="t('finance.newClaim')"
      :width="820"
      :confirm-loading="submitting"
      :ok-button-props="{ disabled: missingNotes > 0 }"
      @ok="submitClaim"
    >
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · finance.claim.create'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.claimType')"><a-select v-model:value="claimForm.type" :options="typeOptions" /></a-form-item>
          <a-form-item :label="t('field.employee')"><a-select v-model:value="claimForm.applicantId" :options="empOptions" /></a-form-item>
          <a-form-item :label="t('field.amount')">
            <!-- 填了明细就由明细合计决定，手填总额禁用，避免对不上账 -->
            <a-input-number v-model:value="claimForm.amount" :min="0" :step="100" :disabled="claimForm.items.length > 0" style="width: 100%" />
          </a-form-item>
          <a-form-item :label="t('field.purpose')"><a-input v-model:value="claimForm.purpose" /></a-form-item>
          <a-form-item :label="t('field.project')"><a-input v-model:value="claimForm.project" :placeholder="t('common.optional')" /></a-form-item>
        </div>

        <a-form-item :label="t('finance.itemsTitle')">
          <div class="item-editor">
            <div class="item-row" v-for="(it, i) in claimForm.items" :key="i">
              <a-select v-model:value="it.category" :options="categoryOptions" style="width: 116px" />
              <a-input-number v-model:value="it.amount" :min="0" :step="10" :placeholder="t('field.amount')" style="width: 116px" />
              <a-tag v-if="isOverStandard(it)" color="orange">{{ t('finance.overStandard') }}</a-tag>
              <a-input v-model:value="it.occurredOn" placeholder="YYYY-MM-DD" style="width: 124px" />
              <a-input v-model:value="it.invoiceNo" :placeholder="t('finance.itemInvoice')" style="width: 130px" />
              <a-input
                v-model:value="it.note"
                :placeholder="isOverStandard(it) ? t('finance.overNotePh') : t('finance.itemNote')"
                :class="{ 'is-over': isOverStandard(it) }"
              />
              <a-button type="text" danger size="small" @click="removeItem(i)">✕</a-button>
            </div>
            <div class="item-foot">
              <a-button size="small" @click="addItem"><Icon name="plus" :size="13" /> {{ t('finance.addItem') }}</a-button>
              <span v-if="claimForm.items.length" class="small muted">
                {{ t('finance.itemTotal') }} <b>{{ money(itemsTotal) }}</b>
              </span>
              <span v-else class="small muted">{{ t('finance.itemEmpty') }}</span>
            </div>
            <div class="standard-tip small muted">{{ t('finance.standardTip') }}：{{ standardTip }}</div>
            <div v-if="missingNotes > 0" class="standard-warn">
              {{ t('finance.overNeedNote', { n: missingNotes }) }}
            </div>
          </div>
        </a-form-item>

        <!-- 填报附件：单号要创建后才有，先暂存，提交成功再随单上传 -->
        <a-form-item :label="t('attach.title')">
          <div class="stage-box">
            <div class="stage-head">
              <a-button size="small" :disabled="submitting" @click="pickFiles">
                <Icon name="plus" :size="13" /> {{ t('attach.add') }}
              </a-button>
              <span class="small muted">{{ t('attach.staged') }}</span>
              <input
                ref="fileInput"
                class="stage-input"
                type="file"
                multiple
                accept="image/png,image/jpeg,image/webp,image/gif,application/pdf"
                @change="onStagedFiles"
              />
            </div>
            <div v-if="!stagedFiles.length" class="small muted">{{ t('attach.empty') }}</div>
            <div v-for="(f, i) in stagedFiles" :key="i" class="stage-row">
              <span class="stage-name">{{ f.name }}</span>
              <span class="small muted">{{ sizeText(f.size) }}</span>
              <span class="spacer"></span>
              <a-button type="link" size="small" danger @click="removeStaged(i)">
                {{ t('attach.remove') }}
              </a-button>
            </div>
          </div>
        </a-form-item>
      </a-form>
    </a-modal>
  </div>
</template>

<style scoped>
.standard-tip {
  margin-top: 6px;
}
.standard-warn {
  margin-top: 4px;
  font-size: 12px;
  color: #d48806;
}
.item-row :deep(.is-over) {
  border-color: #d48806;
}
.stage-box {
  display: flex;
  flex-direction: column;
  gap: 6px;
  width: 100%;
}
.stage-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.stage-input {
  display: none;
}
.stage-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}
.stage-name {
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
