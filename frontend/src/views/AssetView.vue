<script setup lang="ts">
/** 资产：台账 / 登记·领用·归还 / 类别分布 / 盘点原值合计 / 详情 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import ColumnPicker from '../components/ColumnPicker.vue'
import CustomFieldForm from '../components/CustomFieldForm.vue'
import { useColumns } from '../composables/useColumns'
import { useCustomFields } from '../composables/useCustomFields'
import { listTable } from '../api/bridge'
import { ASSET_CATEGORIES, toAsset, toEmployee, type Asset, type Employee } from '../model'
import { aliased, centsArgs, money, tagColor, wan } from '../utils'
import { auth, can, dataVersion, submitTool, takePageFilter } from '../store'

const { t } = useI18n()
const custom = useCustomFields()
const loading = ref(false)
const error = ref('')
const assets = ref<Asset[]>([])
const employees = ref<Employee[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [as, em] = await Promise.all([listTable('assets'), listTable('employees')])
    assets.value = as.map(toAsset)
    employees.value = em.map(toEmployee)
    await custom.reload()
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
// 工作台「资产总值」卡片带过来的筛选：只消费一次
onMounted(() => {
  const filter = takePageFilter('asset')
  if (filter.assetStatus) statusFilter.value = filter.assetStatus
  void load()
})
watch(dataVersion, load)

const categoryFilter = ref('all')
const statusFilter = ref('')
const keyword = ref('')
const categoryOptions = computed(() => [
  { label: t('common.all'), value: 'all' },
  ...ASSET_CATEGORIES.map((k) => ({ label: t('st.' + k), value: k })),
])
const statusOptions = computed(() => [
  { label: t('common.all') + t('common.status'), value: '' },
  // 与工作台「资产总值」卡片同口径：排除已报废
  { label: t('asset.notScrapped'), value: 'active' },
  ...['instock', 'inuse', 'repair', 'scrapped'].map((k) => ({ label: t('st.' + k), value: k })),
])
const empOptions = computed(() =>
  employees.value.filter((e) => e.status !== 'left').map((e) => ({ label: `${e.name} · ${e.no}`, value: e.id })),
)
const inStockOptions = computed(() =>
  assets.value.filter((a) => a.status === 'instock').map((a) => ({ label: `${a.id} · ${a.model}`, value: a.id })),
)
const inUseOptions = computed(() =>
  assets.value.filter((a) => a.status === 'inuse').map((a) => ({ label: `${a.id} · ${a.model}`, value: a.id })),
)
function holderName(id: string): string {
  return employees.value.find((e) => e.id === id)?.name || '—'
}
const filteredAssets = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  return assets.value.filter((a) => {
    if (categoryFilter.value !== 'all' && a.category !== categoryFilter.value) return false
    if (statusFilter.value === 'active') {
      if (a.status === 'scrapped') return false
    } else if (statusFilter.value && a.status !== statusFilter.value) {
      return false
    }
    if (!kw) return true
    return `${a.id} ${a.model} ${a.sn} ${holderName(a.holderId)}`.toLowerCase().includes(kw)
  })
})
const inventoryTotal = computed(() => assets.value.reduce((n, a) => n + a.price, 0))
const assetKpis = computed(() => [
  { label: t('asset.kpiTotal'), value: String(assets.value.length), ico: 'box', color: 'blue' },
  { label: t('asset.kpiInUse'), value: String(assets.value.filter((a) => a.status === 'inuse').length), ico: 'users', color: 'green' },
  { label: t('asset.kpiInStock'), value: String(assets.value.filter((a) => a.status === 'instock').length), ico: 'inbox', color: 'cyan' },
  { label: t('asset.kpiValue'), value: wan(inventoryTotal.value), ico: 'wallet', color: 'purple' },
])
const distRows = computed(() =>
  ASSET_CATEGORIES.map((k) => ({ key: k, n: assets.value.filter((a) => a.category === k).length }))
    .filter((r) => r.n > 0).sort((a, b) => b.n - a.n),
)
const inventoryColumns = computed(() => [
  { title: t('asset.colCategory'), key: 'category', locked: true },
  { title: t('common.total'), key: 'cnt', align: 'right' as const },
  { title: t('asset.colPrice'), key: 'value', align: 'right' as const },
])
const columns = computed(() => [
  { title: t('asset.colNo'), dataIndex: 'id', key: 'id', locked: true },
  { title: t('asset.colCategory'), key: 'category' },
  { title: t('asset.colModel'), dataIndex: 'model', key: 'model' },
  { title: t('asset.colSn'), dataIndex: 'sn', key: 'sn' },
  { title: t('asset.colStatus'), key: 'status' },
  { title: t('asset.colHolder'), key: 'holder' },
  { title: t('asset.colLocation'), dataIndex: 'location', key: 'location' },
  { title: t('asset.colPrice'), key: 'price', align: 'right' as const },
  { title: t('common.action'), key: 'action', locked: true },
])
const assetModel = useColumns('asset.assets', () => columns.value, () => auth.user?.username || '')
const inventoryModel = useColumns('asset.inventory', () => inventoryColumns.value, () => auth.user?.username || '')

/* ---------- 详情 ---------- */
const detailOpen = ref(false)
const detailRow = ref<Asset | null>(null)
const detailRows = computed<{ k: string; v: string }[]>(() => {
  const a = detailRow.value
  if (!a) return []
  return [
    { k: t('asset.colNo'), v: a.id }, { k: t('asset.colCategory'), v: t('st.' + a.category) },
    { k: t('asset.colModel'), v: a.model }, { k: t('asset.colSn'), v: a.sn },
    { k: t('asset.colStatus'), v: t('st.' + a.status) }, { k: t('asset.colHolder'), v: a.holderId ? holderName(a.holderId) : '—' },
    { k: t('asset.colLocation'), v: a.location }, { k: t('asset.colPrice'), v: money(a.price) },
    { k: t('asset.colDate'), v: a.purchaseDate },
  ].concat(
    custom.valuesFor('asset', a.id).map((v) => ({
      k: custom.allFields.value.find((f) => f.id === v.fieldId)?.label ?? v.fieldId,
      v: v.value,
    })),
  )
})
function openDetail(row: Asset): void {
  detailRow.value = row
  detailOpen.value = true
}

/* ---------- 写操作 ---------- */
async function returnAsset(id: string): Promise<void> {
  await submitTool('asset.return', { asset_id: id, condition: 'good' })
}

/** 盘点结果：asset.query 按类别汇总（服务端口径），点「盘点」后展示 */
interface InventoryRow { category: string; cnt: number; value: number }
const inventoryRows = ref<InventoryRow[]>([])
const inventoryValue = ref(0)
async function inventory(): Promise<void> {
  const res = await submitTool('asset.query', { group_by: 'category' })
  const r = res as unknown as {
    by_category?: { category: string; cnt: number; value: number }[]
    total_value_cents?: number
  }
  inventoryRows.value = (r.by_category || []).map((x) => ({
    category: String(x.category), cnt: Number(x.cnt), value: Number(x.value),
  }))
  inventoryValue.value = Number(r.total_value_cents || 0)
}

const regOpen = ref(false)
const assignOpen = ref(false)
const returnOpen = ref(false)
const submitting = ref(false)
const regForm = reactive({ category: 'laptop', model: '', sn: '', price: 5000, location: '' })
const regCustom = ref<Record<string, string>>({})
const regCustomRef = ref<InstanceType<typeof CustomFieldForm> | null>(null)
const assignForm = reactive({ asset_id: '', employeeId: '' })
const returnForm = reactive({ asset_id: '' })
function openReg(): void {
  Object.assign(regForm, { category: 'laptop', model: '', sn: '', price: 5000, location: '' })
  regCustom.value = custom.emptyValues('asset')
  regOpen.value = true
}
function openAssign(): void {
  Object.assign(assignForm, { asset_id: inStockOptions.value[0]?.value || '', employeeId: employees.value[0]?.id || '' })
  assignOpen.value = true
}
function openReturn(): void {
  Object.assign(returnForm, { asset_id: inUseOptions.value[0]?.value || '' })
  returnOpen.value = true
}
async function submitReg(): Promise<void> {
  if (regCustomRef.value && !regCustomRef.value.validate()) {
    message.warning(t('custom.requiredMissing'))
    return
  }
  submitting.value = true
  try {
    const args = {
      category: regForm.category,
      model: regForm.model.trim() || 'IT Equipment',
      sn: regForm.sn.trim(),
      location: regForm.location.trim() || '—',
      ...centsArgs('price', regForm.price),
      custom_fields: custom.submissionValues('asset', regCustom.value),
    }
    const res = await submitTool('asset.register', args)
    if (res.ok) regOpen.value = false
  } finally { submitting.value = false }
}
async function submitAssign(): Promise<void> {
  submitting.value = true
  try {
    const args = aliased({ asset_id: assignForm.asset_id, employeeId: assignForm.employeeId })
    const res = await submitTool('asset.assign', args)
    if (res.ok) assignOpen.value = false
  } finally { submitting.value = false }
}
async function submitReturn(): Promise<void> {
  submitting.value = true
  try {
    const res = await submitTool('asset.return', { asset_id: returnForm.asset_id, condition: 'good' })
    if (res.ok) returnOpen.value = false
  } finally { submitting.value = false }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('asset.title') }}</h1><p>{{ t('asset.desc') }}</p></div>
      <span class="spacer"></span>
      <div class="oa-head-actions">
        <a-button @click="inventory"><Icon name="pie" :size="14" /> {{ t('asset.inventory') }}</a-button>
        <a-button v-if="can('asset.return')" @click="openReturn"><Icon name="refresh" :size="14" /> {{ t('asset.return') }}</a-button>
        <a-button v-if="can('asset.assign')" @click="openAssign"><Icon name="user" :size="14" /> {{ t('asset.assign') }}</a-button>
        <a-button v-if="can('asset.register')" type="primary" @click="openReg"><Icon name="plus" :size="14" /> {{ t('asset.newAsset') }}</a-button>
      </div>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <div class="oa-grid g-4 mb16">
        <div class="stat-card" v-for="k in assetKpis" :key="k.label">
          <div class="stat-ico" :class="'bg-' + k.color"><Icon :name="k.ico" :size="18" /></div>
          <div class="stat-label">{{ k.label }}</div>
          <a-statistic :value="k.value" :value-style="{ fontSize: '23px', fontWeight: 700 }" />
        </div>
      </div>

      <a-card class="mb16">
        <div class="oa-toolbar">
          <a-select v-model:value="categoryFilter" :options="categoryOptions" style="width: 150px" />
          <a-select v-model:value="statusFilter" :options="statusOptions" style="width: 150px" />
          <a-input v-model:value="keyword" :placeholder="t('asset.searchPh')" allow-clear style="width: 230px" />
          <span class="spacer"></span>
          <span class="small muted">{{ t('asset.totalValue') }}: <b>{{ money(inventoryTotal) }}</b></span>
          <ColumnPicker :model="assetModel" />
        </div>
        <a-table class="oa-table" :columns="assetModel.columns" :data-source="filteredAssets" row-key="id" :pagination="{ pageSize: 8 }">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'id'"><span class="cell-link" @click="openDetail(record)">{{ record.id }}</span></template>
            <template v-else-if="column.key === 'category'"><a-tag :color="tagColor(record.category)">{{ t('st.' + record.category) }}</a-tag></template>
            <template v-else-if="column.key === 'status'"><a-tag :color="tagColor(record.status)">{{ t('st.' + record.status) }}</a-tag></template>
            <template v-else-if="column.key === 'holder'">{{ record.holderId ? holderName(record.holderId) : '—' }}</template>
            <template v-else-if="column.key === 'price'"><span class="cell-num">{{ money(record.price) }}</span></template>
            <template v-else-if="column.key === 'action'">
              <div class="row-actions">
                <a-button v-if="record.status === 'inuse' && can('asset.return')" size="small" @click="returnAsset(record.id)">{{ t('asset.return') }}</a-button>
                <a-button type="link" size="small" @click="openDetail(record)">{{ t('common.view') }}</a-button>
              </div>
            </template>
          </template>
        </a-table>
      </a-card>

      <a-card :title="t('asset.distTitle')">
        <template #extra><span class="small muted">{{ t('asset.kpiTotal') }} {{ assets.length }}</span></template>
        <div style="display: flex; flex-wrap: wrap; gap: 16px">
          <div v-for="r in distRows" :key="r.key" style="width: 30%; min-width: 200px">
            <div style="display: flex; justify-content: space-between; font-size: 12.5px; margin-bottom: 5px">
              <span>{{ t('st.' + r.key) }}</span><b>{{ r.n }}</b>
            </div>
            <a-progress :percent="Math.round(r.n / Math.max(1, assets.length) * 100)" />
          </div>
        </div>
      </a-card>

      <!-- 盘点结果（点「盘点」后按类别汇总） -->
      <a-card v-if="inventoryRows.length" :title="t('asset.inventoryTitle')" class="mt16">
        <template #extra>
          <div style="display: flex; align-items: center; gap: 8px">
            <span class="small muted">{{ t('asset.totalValue') }}: <b>{{ money(inventoryValue) }}</b></span>
            <ColumnPicker :model="inventoryModel" />
          </div>
        </template>
        <a-table class="oa-table" :columns="inventoryModel.columns" :data-source="inventoryRows" row-key="category" :pagination="false" size="small">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'category'">{{ t('st.' + record.category) }}</template>
            <template v-else-if="column.key === 'value'"><span class="cell-num">{{ money(record.value) }}</span></template>
          </template>
        </a-table>
      </a-card>
    </AsyncPanel>

    <!-- 详情 -->
    <a-drawer v-model:open="detailOpen" :title="t('asset.detailTitle')" :width="440">
      <div class="desc-grid">
        <div class="desc-item" v-for="(r, i) in detailRows" :key="i">
          <span class="desc-k">{{ r.k }}</span><span class="desc-v">{{ r.v }}</span>
        </div>
      </div>
      <div v-if="detailRow?.qrCode" class="qr-box">
        <div v-if="detailRow.qrCode.trim().startsWith('<svg')" v-html="detailRow.qrCode" class="qr-svg"></div>
        <span v-else class="small muted">{{ detailRow.qrCode }}</span>
        <div class="small muted mt8">{{ t('asset.qrHint') }}</div>
      </div>
    </a-drawer>

    <!-- 登记入库 -->
    <a-modal v-model:open="regOpen" :title="t('asset.newAsset')" :confirm-loading="submitting" @ok="submitReg">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · asset.register'" />
      <a-form layout="vertical">
        <div class="oa-form-grid">
          <a-form-item :label="t('field.assetCategory')"><a-select v-model:value="regForm.category" :options="ASSET_CATEGORIES.map(k => ({ label: t('st.' + k), value: k }))" /></a-form-item>
          <a-form-item :label="t('field.model')"><a-input v-model:value="regForm.model" /></a-form-item>
          <a-form-item :label="t('field.sn')"><a-input v-model:value="regForm.sn" /></a-form-item>
          <a-form-item :label="t('field.price')"><a-input-number v-model:value="regForm.price" :min="0" :step="500" style="width: 100%" /></a-form-item>
          <a-form-item :label="t('field.location')"><a-input v-model:value="regForm.location" /></a-form-item>
        </div>
        <CustomFieldForm
          ref="regCustomRef"
          entity-type="asset"
          :values="regCustom"
          @update:values="regCustom = $event"
        />
      </a-form>
    </a-modal>

    <!-- 领用 -->
    <a-modal v-model:open="assignOpen" :title="t('asset.assignTitle')" :confirm-loading="submitting" @ok="submitAssign">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · asset.assign'" />
      <a-form layout="vertical">
        <a-form-item :label="t('field.asset')"><a-select v-model:value="assignForm.asset_id" :options="inStockOptions" /></a-form-item>
        <a-form-item :label="t('field.employee')"><a-select v-model:value="assignForm.employeeId" :options="empOptions" /></a-form-item>
      </a-form>
    </a-modal>

    <!-- 归还 -->
    <a-modal v-model:open="returnOpen" :title="t('asset.returnTitle')" :confirm-loading="submitting" @ok="submitReturn">
      <a-alert type="info" show-icon class="form-hint" :message="t('common.byAgent') + ' · asset.return'" />
      <a-form layout="vertical">
        <a-form-item :label="t('field.asset')"><a-select v-model:value="returnForm.asset_id" :options="inUseOptions" /></a-form-item>
      </a-form>
    </a-modal>
  </div>
</template>
