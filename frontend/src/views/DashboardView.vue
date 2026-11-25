<script setup lang="ts">
/** 工作台：KPI(getMetrics) / 费用趋势(纯 CSS) / 待办审批 / 合同到期预警 / Agent 活动(getAudit) */
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import {
  getAudit, getMetrics, listTable, resetSeed,
  type AuditEvent, type Metrics,
} from '../api/bridge'
import { QUICK_GROUPS, toClaim, toContract, type Claim, type Contract } from '../model'
import { dateAdd, L, money, today, wan } from '../utils'
import {
  app, auth, bump, can, dataVersion, expandFullscreen, gotoPage, runCommand, submitTool,
} from '../store'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const metrics = ref<Metrics | null>(null)
const claims = ref<Claim[]>([])
const contracts = ref<Contract[]>([])
const audit = ref<AuditEvent[]>([])
const isAdmin = computed(() => auth.user?.role === 'admin')

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [m, cl, co, au] = await Promise.all([
      getMetrics(),
      listTable('claims'),
      listTable('contracts'),
      // 审计日志含入参与结果，后端 /api/audit 仅管理员可读；其他角色不请求，避免整页报错
      isAdmin.value ? getAudit() : Promise.resolve<AuditEvent[]>([]),
    ])
    metrics.value = m
    claims.value = cl.map(toClaim)
    contracts.value = co.map(toContract)
    audit.value = au
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)
watch(dataVersion, load)

const m = computed(() => metrics.value || ({} as Metrics))
const budgetUsed = computed(() => Number(m.value.budget_used_cents || 0) / 100)
const budgetTotal = computed(() => Number(m.value.budget_total_cents || 0) / 100)
const budgetPct = computed(() =>
  budgetTotal.value ? Math.min(100, Math.round((budgetUsed.value / budgetTotal.value) * 100)) : 0,
)
const attendanceRate = computed(() => {
  const r = Number(m.value.attendance_rate || 0)
  return r <= 1 ? r * 100 : r
})
const claimTotal = computed(() =>
  claims.value.filter((c) => c.status !== 'rejected').reduce((n, c) => n + c.amount, 0),
)
const assetValue = computed(() => Number(m.value.asset_value_cents || 0) / 100)
const payrollCents = computed(() => Number(m.value.payroll_cents || 0) / 100)
// 人力成本/预算/费用趋势只对财务、经理与管理员开放（后端对其余角色已置零）
const showFinance = computed(() => ['admin', 'finance', 'manager'].includes(auth.user?.role ?? ''))
const trend = computed(() => (Array.isArray(m.value.trend) ? m.value.trend.map(Number) : []))
const maxTrend = computed(() => Math.max(1, ...trend.value))
function barH(v: number): string {
  return Math.max(3, Math.round((Number(v) / maxTrend.value) * 130)) + 'px'
}

const monthNow = today().slice(0, 7)
// 每张卡片点击后落到对应页面并带上筛选，筛选口径与卡片的统计口径保持一致
const kpis = computed(() => [
  {
    key: 'kpiStaff', ico: 'users', color: 'blue', value: String(Number(m.value.active_employees || 0)),
    unit: t('common.people'), foot: t('dashboard.footStaff'),
    go: () => gotoPage('hr', { employeeStatus: 'working' }),
  },
  {
    key: 'kpiApproval', ico: 'inbox', color: 'orange', value: String(Number(m.value.pending_claims || 0)),
    unit: t('common.result'), foot: t('dashboard.footApproval'),
    go: () => gotoPage('finance', { claimsStatus: 'pending' }),
  },
  {
    key: 'kpiContract', ico: 'file', color: 'cyan', value: String(Number(m.value.expiring_contracts || 0)),
    unit: t('common.result'), foot: t('dashboard.footContract'),
    go: () => gotoPage('contract', { contractsExpiring: true }),
  },
  {
    key: 'kpiAttendance', ico: 'clock', color: 'green', value: attendanceRate.value.toFixed(0) + '%',
    unit: '', foot: t('dashboard.footAttendance'),
    go: () => gotoPage('attendance', { attendanceMonth: monthNow }),
  },
  {
    key: 'kpiAsset', ico: 'box', color: 'red', value: wan(assetValue.value), unit: '',
    foot: t('dashboard.footAsset'),
    go: () => gotoPage('asset', { assetStatus: 'active' }),
  },
  {
    key: 'kpiClaim', ico: 'wallet', color: 'purple', value: wan(claimTotal.value), unit: '',
    foot: t('dashboard.footClaim') + ' ' + budgetPct.value + '%',
    go: () => gotoPage('finance', { claimTab: 'budget' }),
  },
])

const typeRows = computed(() => [
  { key: 'daily', color: '#1677ff', value: claims.value.filter((c) => c.type === 'daily' && c.status !== 'rejected').reduce((n, c) => n + c.amount, 0) },
  { key: 'business', color: '#722ed1', value: claims.value.filter((c) => c.type === 'business' && c.status !== 'rejected').reduce((n, c) => n + c.amount, 0) },
  { key: 'project', color: '#13c2c2', value: claims.value.filter((c) => c.type === 'project' && c.status !== 'rejected').reduce((n, c) => n + c.amount, 0) },
])
const pendingClaims = computed(() => claims.value.filter((c) => c.status === 'pending').slice(0, 5))
const expiringContracts = computed(() =>
  contracts.value.filter(
    (c) =>
      c.status !== 'archived' &&
      (c.status === 'expiring' ||
        (c.expireDate !== '—' && c.expireDate <= dateAdd(30) && c.expireDate >= today())),
  ),
)
const recentAudit = computed(() => audit.value.slice(0, 6))
const quickChips = computed(() =>
  QUICK_GROUPS.map((g) => ({
    icon: g.icon,
    groupLabel: g.label,
    itemLabel: g.items[0]?.label ?? { zh: '', en: '' },
    cmd: g.items[0]?.cmd ?? { zh: '', en: '' },
  })),
)

function short(v: unknown): string {
  const s = L(v)
  return s.length > 64 ? s.slice(0, 64) + '…' : s
}
async function approveClaim(id: string, decision: 'approve' | 'reject'): Promise<void> {
  await submitTool('finance.claim.approve', { claim_id: id, decision, comment: '' })
}
async function resetDemo(): Promise<void> {
  try {
    await resetSeed()
    message.success(t('toast.reset'))
    bump()
  } catch (e) {
    message.error(t('error.requestFailed') + ': ' + String(e))
  }
}
function goAgent(): void {
  expandFullscreen()
}
function goFinance(): void {
  app.activePage = 'finance'
}
function runQuick(cmd: { zh: string; en: string }): void {
  runCommand(L(cmd))
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div>
        <h1>{{ t('dashboard.title') }}</h1>
        <p>{{ t('dashboard.desc') }}</p>
      </div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button @click="load"><Icon name="refresh" :size="14" /> {{ t('common.refresh') }}</a-button>
        <a-button v-if="auth.user?.role === 'admin'" @click="resetDemo"><Icon name="zap" :size="14" /> {{ t('common.reset') }}</a-button>
        <a-button type="primary" @click="goAgent"><Icon name="bot" :size="14" /> {{ t('dashboard.agentEntry') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <!-- 合同到期预警：需要马上处理，放在最前面 -->
      <a-alert
        v-if="expiringContracts.length" class="mb16" type="warning" show-icon
        :message="t('contract.warnTitle')"
      >
        <template #description>
          {{ t('contract.warnDesc') }}
          <code class="tool-name" v-for="c in expiringContracts" :key="c.id" style="margin-left: 6px">
            {{ c.id }} {{ c.expireDate }}
          </code>
        </template>
      </a-alert>

      <!-- KPI 卡片 -->
      <div class="oa-grid g-6 mb16">
        <div
          class="stat-card is-clickable" v-for="k in kpis" :key="k.key"
          role="button" tabindex="0" :title="t('dashboard.cardHint')"
          @click="k.go" @keydown.enter.prevent="k.go" @keydown.space.prevent="k.go"
        >
          <Icon name="arrow" :size="12" class="stat-go" />
          <div class="stat-row">
            <div class="stat-ico" :class="'bg-' + k.color"><Icon :name="k.ico" :size="18" /></div>
            <a-statistic :value="k.value" :suffix="k.unit" :value-style="{ fontSize: '23px', fontWeight: 700 }" />
          </div>
          <div class="stat-label">{{ t('dashboard.' + k.key) }}</div>
          <div class="stat-foot">{{ k.foot }}</div>
        </div>
      </div>

      <!-- 费用趋势 + 预算执行 / 类型分布 -->
      <div class="oa-grid g-2-1 mb16">
        <a-card :title="t('dashboard.trend')">
          <template #extra>
            <span v-if="showFinance" class="small muted">
              {{ t('dashboard.payroll') }} <b>{{ money(payrollCents) }}</b>
            </span>
          </template>
          <div v-if="showFinance" class="trend-wrap">
            <div v-if="!trend.length" class="trend-empty">{{ t('dashboard.trendEmpty') }}</div>
            <div v-else class="trend-chart">
              <div class="trend-col" v-for="(v, i) in trend" :key="i">
                <div class="trend-bars">
                  <div class="trend-bar" :style="{ height: barH(v) }" :title="t('dashboard.reimburse') + ' ' + v"></div>
                </div>
                <div class="trend-label">{{ i + 1 }}</div>
              </div>
            </div>
            <div class="small muted" style="padding: 8px">{{ t('dashboard.trendSub') }}</div>
          </div>
          <a-empty v-else :description="t('dashboard.financeOnly')" />
        </a-card>

        <a-card :title="t('finance.budgetTitle')">
          <template v-if="showFinance">
            <p class="small muted">{{ t('finance.budgetDesc') }}</p>
            <div style="display: flex; justify-content: space-between; margin: 10px 0">
              <span class="small">{{ t('finance.used') }} <b>{{ money(budgetUsed) }}</b></span>
              <span class="small">{{ t('finance.remain') }} <b>{{ money(budgetTotal - budgetUsed) }}</b></span>
            </div>
            <a-progress :percent="budgetPct" />
            <div class="small muted" style="margin-top: 10px">
              {{ budgetPct }}% · {{ money(budgetTotal) }} {{ t('finance.budgetRate') }}
            </div>
            <a-divider style="margin: 14px 0" />
            <div class="small" style="margin-bottom: 8px"><b>{{ t('dashboard.typeSplit') }}</b> <span class="muted">· {{ t('dashboard.typeSplitSub') }}</span></div>
            <div class="donut-row" v-for="r in typeRows" :key="r.key">
              <i :style="{ background: r.color }"></i>
              <span>{{ t('st.' + r.key) }}</span>
              <span class="spacer"></span>
              <b>{{ money(r.value) }}</b>
            </div>
          </template>
          <a-empty v-else :description="t('dashboard.financeOnly')" />
        </a-card>
      </div>

      <!-- 待办审批 + Agent 活动 -->
      <div class="oa-grid g-2-1 mb16">
        <a-card :title="t('dashboard.todo')">
          <template #extra>
            <a-button type="link" size="small" @click="goFinance">{{ t('dashboard.viewAll') }}</a-button>
          </template>
          <ul class="tl-list" v-if="pendingClaims.length">
            <li class="tl-item" v-for="c in pendingClaims" :key="c.id">
              <span class="tl-dot" :style="{ background: c.type === 'project' ? '#13c2c2' : c.type === 'business' ? '#722ed1' : '#1677ff' }"></span>
              <div class="tl-body">
                <div class="tl-title">
                  <b>{{ c.id }}</b> · {{ c.purpose }} ·
                  <span style="color: #1677ff; font-weight: 600">{{ money(c.amount) }}</span>
                </div>
                <div class="tl-time">{{ c.submitDate }} · {{ t('st.' + c.type) }} · {{ c.applicantId }}</div>
              </div>
              <div v-if="can('finance.claim.approve')" class="row-actions">
                <a-button size="small" @click="approveClaim(c.id, 'approve')">{{ t('finance.approve') }}</a-button>
                <a-button size="small" danger @click="approveClaim(c.id, 'reject')">{{ t('finance.reject') }}</a-button>
              </div>
            </li>
          </ul>
          <a-empty v-else :description="t('common.noData')" />
        </a-card>

        <a-card :title="t('dashboard.activity')">
          <template #extra><span class="small muted">{{ t('dashboard.activitySub') }}</span></template>
          <ul class="tl-list" v-if="recentAudit.length">
            <li class="tl-item" v-for="a in recentAudit" :key="a.id">
              <span class="tl-dot" style="background: #722ed1"></span>
              <div class="tl-body">
                <div class="tl-title">{{ short(a.payload ?? a.tool) }}</div>
                <div class="tl-time">
                  <code class="tool-name">{{ a.tool }}</code> · {{ a.actor }} · {{ a.created_at }}
                </div>
              </div>
            </li>
          </ul>
          <a-empty
            v-else
            :description="isAdmin ? t('dashboard.emptyActivity') : t('dashboard.activityAdminOnly')"
          />
        </a-card>
      </div>

      <!-- 快捷指令 -->
      <a-card :title="t('dashboard.quick')">
        <template #extra><span class="small muted">{{ t('dashboard.quickSub') }}</span></template>
        <div style="display: flex; gap: 8px; flex-wrap: wrap; align-items: center">
          <button class="agent-chip" v-for="(ch, i) in quickChips" :key="i" @click="runQuick(ch.cmd)">
            <Icon :name="ch.icon" :size="12" /> {{ L(ch.groupLabel) }} · {{ L(ch.itemLabel) }}
          </button>
          <a-button type="primary" size="small" style="margin-left: auto" @click="goAgent">
            {{ t('dashboard.agentEntry') }} <Icon name="arrow" :size="13" />
          </a-button>
        </div>
      </a-card>
    </AsyncPanel>
  </div>
</template>
