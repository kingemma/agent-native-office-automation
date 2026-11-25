<script setup lang="ts">
/** 设置：账户（改密）/ 模型（管理员）/ 自定义字段库（管理员）/ 服务端地址（只读） */
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { message, Modal } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import LlmSettingsModal from '../components/LlmSettingsModal.vue'
import {
  callTool, changePassword, getAttendancePolicy, getAttendanceSites, getMakeupQuota, listTable,
  saveAttendancePolicy, saveAttendanceSites, saveMakeupQuota,
  type AttendancePolicy, type AttendanceSite,
} from '../api/bridge'
import { getCurrentLocation, GeoError } from '../composables/useGeolocation'
import { useCustomFields } from '../composables/useCustomFields'
import {
  toAttendanceExemption, toDepartment, toEmployee,
  type AttendanceExemption, type CustomEntityType, type CustomFieldType, type Department,
  type Employee,
} from '../model'
import { auth, doLogout, endpoint } from '../store'

const { t } = useI18n()
const user = computed(() => auth.user)
const llmOpen = ref(false)
const isAdmin = computed(() => user.value?.role === 'admin')

const { allFields, reload: reloadFields } = useCustomFields()
const fieldModalOpen = ref(false)
const fieldEditing = ref(false)
const fieldForm = reactive({
  id: '',
  entityType: 'employee' as CustomEntityType,
  labelZh: '',
  labelEn: '',
  fieldType: 'text' as CustomFieldType,
  optionsText: '',
  required: false,
  sortOrder: 0,
})

const ENTITY_TYPES: CustomEntityType[] = ['employee', 'claim', 'contract', 'asset']
const FIELD_TYPES: CustomFieldType[] = ['text', 'number', 'date', 'select']

function openFieldForm(field?: (typeof allFields.value)[number]): void {
  fieldEditing.value = !!field
  let labelZh = ''
  let labelEn = ''
  if (field) {
    // toCustomField 已折成当前语言；这里尽量还原双语用于编辑
    try {
      const raw = JSON.parse(field.label)
      labelZh = raw.zh ?? ''
      labelEn = raw.en ?? ''
    } catch {
      labelZh = field.label
      labelEn = field.label
    }
  }
  Object.assign(fieldForm, {
    id: field?.id ?? '',
    entityType: field?.entityType ?? 'employee',
    labelZh,
    labelEn,
    fieldType: field?.fieldType ?? 'text',
    optionsText: field?.options.join(', ') ?? '',
    required: field?.required ?? false,
    sortOrder: field?.sortOrder ?? 0,
  })
  fieldModalOpen.value = true
}

async function saveField(): Promise<void> {
  if (!fieldForm.id.trim().startsWith('cf_')) {
    message.warning(t('custom.idHint'))
    return
  }
  if (!fieldForm.labelZh.trim()) {
    message.warning(t('custom.labelRequired'))
    return
  }
  const options = fieldForm.optionsText
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean)
  if (fieldForm.fieldType === 'select' && options.length === 0) {
    message.warning(t('custom.optionsRequired'))
    return
  }
  try {
    const out = await callTool('custom.field.define', {
      field_id: fieldForm.id.trim(),
      entity_type: fieldForm.entityType,
      label: JSON.stringify({
        zh: fieldForm.labelZh.trim(),
        en: fieldForm.labelEn.trim() || fieldForm.labelZh.trim(),
      }),
      field_type: fieldForm.fieldType,
      options: JSON.stringify(options),
      required: fieldForm.required,
      sort_order: fieldForm.sortOrder,
    })
    if (!out.ok) throw new Error(out.msg || out.error || t('common.failed'))
    message.success(t('custom.saved'))
    fieldModalOpen.value = false
    await reloadFields()
  } catch (e) {
    message.error(String(e))
  }
}

async function removeField(field: (typeof allFields.value)[number]): Promise<void> {
  Modal.confirm({
    title: t('custom.removeTitle'),
    content: t('custom.removeConfirm', { label: field.label }),
    okType: 'danger',
    async onOk() {
      try {
        const out = await callTool('custom.field.remove', { field_id: field.id })
        if (!out.ok) throw new Error(out.msg || out.error || t('common.failed'))
        message.success(t('custom.removed'))
        await reloadFields()
      } catch (e) {
        message.error(String(e))
      }
    },
  })
}

/* ---------- 打卡设置（仅管理员）：多点 + 每点独立半径 ---------- */
interface AttendanceSiteForm extends Omit<AttendanceSite, 'latitude' | 'longitude'> {
  latitude?: number
  longitude?: number
}

const attendanceSites = ref<AttendanceSiteForm[]>([])
const attendanceDefaultRadius = ref(2000)
const attendanceSaving = ref(false)

async function loadAttendanceSites(): Promise<void> {
  try {
    const { sites, default_radius_m } = await getAttendanceSites()
    attendanceDefaultRadius.value = default_radius_m
    attendanceSites.value = sites.map((site) => ({ ...site }))
  } catch (e) {
    message.error(String(e))
  }
}

function addAttendanceSite(): void {
  attendanceSites.value.push({
    name: '',
    radius_m: attendanceDefaultRadius.value,
    address: '',
  })
}

function removeAttendanceSite(index: number): void {
  attendanceSites.value.splice(index, 1)
}

async function useCurrentLocation(index: number): Promise<void> {
  const site = attendanceSites.value[index]
  if (!site) return
  try {
    const point = await getCurrentLocation()
    site.latitude = Number(point.latitude.toFixed(6))
    site.longitude = Number(point.longitude.toFixed(6))
  } catch (e) {
    message.error(e instanceof GeoError ? t(`attendance.geo.${e.kind}`) : String(e))
  }
}

/* ---------- 每月补卡次数：全局默认 + 员工覆盖 ---------- */
const employees = ref<Employee[]>([])
const empOptions = computed(() =>
  employees.value.map((e) => ({ label: `${e.name} · ${e.id}`, value: e.id })),
)
const makeupDefault = ref(3)
const makeupOverrides = ref<{ employee_id: string; makeup_quota: number }[]>([])
const overrideForm = reactive({ employeeId: '', quota: 3 })

async function loadMakeupQuota(): Promise<void> {
  try {
    const info = await getMakeupQuota()
    makeupDefault.value = info.default_quota
    makeupOverrides.value = info.overrides
  } catch (e) {
    message.error(String(e))
  }
}

async function saveMakeupDefault(): Promise<void> {
  try {
    await saveMakeupQuota(makeupDefault.value)
    await loadMakeupQuota()
    message.success(t('settings.makeupQuotaSaved'))
  } catch (e) {
    message.error(String(e))
  }
}

async function saveMakeupOverride(): Promise<void> {
  if (!overrideForm.employeeId) {
    message.warning(t('settings.makeupQuotaPickEmployee'))
    return
  }
  try {
    await saveMakeupQuota(overrideForm.quota, overrideForm.employeeId)
    await loadMakeupQuota()
    message.success(t('settings.makeupQuotaSaved'))
  } catch (e) {
    message.error(String(e))
  }
}

/* ---------- 考勤规则：上下班时间 + 免考勤 ---------- */
const policyForm = reactive<AttendancePolicy>({
  work_start: '09:00',
  work_end: '18:00',
  lunch_minutes: 60,
})
const policySaving = ref(false)
const departments = ref<Department[]>([])
const deptOptions = computed(() =>
  departments.value.map((d) => ({ label: d.name, value: d.id })),
)
const exemptions = ref<AttendanceExemption[]>([])
const exemptScope = ref<'employee' | 'department'>('employee')
const exemptTarget = ref('')

async function loadPolicy(): Promise<void> {
  try {
    const { policy } = await getAttendancePolicy()
    Object.assign(policyForm, policy)
    exemptions.value = (await listTable('attendance_exemptions')).map(toAttendanceExemption)
  } catch (e) {
    message.error(String(e))
  }
}

async function savePolicy(): Promise<void> {
  policySaving.value = true
  try {
    await saveAttendancePolicy({ ...policyForm })
    message.success(t('settings.policySaved'))
  } catch (e) {
    message.error(String(e))
  } finally {
    policySaving.value = false
  }
}

async function addExemption(): Promise<void> {
  if (!exemptTarget.value) {
    message.warning(t('settings.exemptPickTarget'))
    return
  }
  try {
    const out = await callTool('hr.attendance.exempt', {
      scope: exemptScope.value,
      target_id: exemptTarget.value,
    })
    if (!out.ok) throw new Error(out.msg || out.error || t('common.failed'))
    exemptTarget.value = ''
    await loadPolicy()
    message.success(t('settings.exemptSaved'))
  } catch (e) {
    message.error(String(e))
  }
}

async function removeExemption(row: AttendanceExemption): Promise<void> {
  try {
    const out = await callTool('hr.attendance.exempt', {
      scope: row.scope,
      target_id: row.targetId,
      exempt: false,
    })
    if (!out.ok) throw new Error(out.msg || out.error || t('common.failed'))
    await loadPolicy()
  } catch (e) {
    message.error(String(e))
  }
}

async function saveAttendance(): Promise<void> {
  const sites: AttendanceSite[] = []
  for (const item of attendanceSites.value) {
    if (item.latitude == null || item.longitude == null) {
      message.warning(t('settings.attendanceCoordRequired'))
      return
    }
    sites.push({
      name: item.name.trim(),
      latitude: item.latitude,
      longitude: item.longitude,
      radius_m: item.radius_m,
      address: item.address.trim(),
    })
  }
  attendanceSaving.value = true
  try {
    await saveAttendanceSites(sites)
    message.success(t('settings.attendanceSaved'))
  } catch (e) {
    message.error(String(e))
  } finally {
    attendanceSaving.value = false
  }
}

onMounted(() => {
  if (isAdmin.value) {
    void reloadFields()
    void loadAttendanceSites()
    void loadMakeupQuota()
    void loadPolicy()
    void listTable('employees')
      .then((rows) => { employees.value = rows.map(toEmployee) })
      .catch(() => {})
    void listTable('departments')
      .then((rows) => { departments.value = rows.map(toDepartment) })
      .catch(() => {})
  }
})

const pwdForm = reactive({ old: '', next: '', confirm: '' })
const pwdLoading = ref(false)

async function submitPassword(): Promise<void> {
  if (pwdForm.next !== pwdForm.confirm) {
    message.warning(t('auth.passwordMismatch'))
    return
  }
  pwdLoading.value = true
  try {
    await changePassword(pwdForm.old, pwdForm.next)
    message.success(t('auth.passwordChanged'))
    await doLogout()
  } catch (e) {
    message.error(String(e))
  } finally {
    pwdLoading.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('settings.title') }}</h1><p>{{ t('settings.desc') }}</p></div>
    </div>

    <div class="oa-grid g-2-1">
      <a-card :title="t('settings.account')">
        <a-form layout="vertical" style="max-width: 360px">
          <a-form-item :label="t('auth.oldPassword')">
            <a-input-password v-model:value="pwdForm.old" autocomplete="current-password" />
          </a-form-item>
          <a-form-item :label="t('auth.newPassword')">
            <a-input-password v-model:value="pwdForm.next" autocomplete="new-password" />
          </a-form-item>
          <a-form-item :label="t('auth.newPassword')">
            <a-input-password v-model:value="pwdForm.confirm" autocomplete="new-password" />
          </a-form-item>
          <a-button type="primary" :loading="pwdLoading" @click="submitPassword">
            {{ t('auth.changePassword') }}
          </a-button>
        </a-form>
      </a-card>

      <div>
        <a-card :title="t('settings.model')" class="mb16">
          <template #extra>
            <a-tag v-if="user?.role === 'admin'" color="blue">{{ t('settings.adminOnly') }}</a-tag>
          </template>
          <p class="small muted">{{ t('settings.modelDesc') }}</p>
          <a-button v-if="user?.role === 'admin'" @click="llmOpen = true">
            <Icon name="settings" :size="14" /> {{ t('llm.entry') }}
          </a-button>
          <a-empty v-else :description="t('settings.modelForbidden')" :image="false" />
        </a-card>

        <a-card v-if="isAdmin" :title="t('settings.attendanceTitle')" class="mb16">
          <template #extra><a-tag color="blue">{{ t('settings.adminOnly') }}</a-tag></template>
          <p class="small muted">{{ t('settings.attendanceDesc') }}</p>
          <a-empty
            v-if="!attendanceSites.length"
            :description="t('settings.attendanceEmpty')"
            :image="false"
          />
          <div v-for="(site, index) in attendanceSites" :key="index">
            <a-divider v-if="index > 0" style="margin: 4px 0 14px" />
            <div style="display: flex; align-items: center; justify-content: space-between">
              <span class="small">{{ t('settings.attendanceSiteN', { n: index + 1 }) }}</span>
              <a-button type="link" size="small" danger @click="removeAttendanceSite(index)">
                {{ t('settings.attendanceRemove') }}
              </a-button>
            </div>
            <a-form layout="vertical" style="max-width: 520px">
              <a-form-item :label="t('settings.attendanceName')">
                <a-input
                  v-model:value="site.name"
                  :placeholder="t('settings.attendanceNamePlaceholder')"
                />
              </a-form-item>
              <a-form-item :label="t('settings.attendanceAddress')">
                <a-input
                  v-model:value="site.address"
                  :placeholder="t('settings.attendanceAddressPlaceholder')"
                />
              </a-form-item>
              <a-form-item :label="t('settings.attendanceCoord')">
                <a-space wrap>
                  <a-input-number
                    v-model:value="site.latitude"
                    :min="-90"
                    :max="90"
                    :step="0.000001"
                    :placeholder="t('settings.attendanceLat')"
                    style="width: 170px"
                  />
                  <a-input-number
                    v-model:value="site.longitude"
                    :min="-180"
                    :max="180"
                    :step="0.000001"
                    :placeholder="t('settings.attendanceLng')"
                    style="width: 170px"
                  />
                  <a-button @click="useCurrentLocation(index)">
                    {{ t('settings.attendanceUseCurrent') }}
                  </a-button>
                </a-space>
              </a-form-item>
              <a-form-item :label="t('settings.attendanceRadius')">
                <a-space>
                  <a-input-number
                    v-model:value="site.radius_m"
                    :min="50"
                    :max="100000"
                    :step="100"
                    style="width: 140px"
                  />
                  <span class="small muted">{{ t('settings.attendanceRadiusHint') }}</span>
                </a-space>
              </a-form-item>
            </a-form>
          </div>
          <a-space wrap>
            <a-button @click="addAttendanceSite">{{ t('settings.attendanceAdd') }}</a-button>
            <a-button type="primary" :loading="attendanceSaving" @click="saveAttendance">
              {{ t('settings.attendanceSave') }}
            </a-button>
          </a-space>

          <a-divider style="margin: 14px 0" />

          <a-form layout="vertical" style="max-width: 460px">
            <a-form-item :label="t('settings.makeupQuotaDefault')">
              <a-space>
                <a-input-number v-model:value="makeupDefault" :min="0" :max="100" style="width: 120px" />
                <a-button @click="saveMakeupDefault">{{ t('settings.makeupQuotaSave') }}</a-button>
              </a-space>
            </a-form-item>
            <a-form-item :label="t('settings.makeupQuotaOverrides')">
              <a-space wrap>
                <a-select
                  v-model:value="overrideForm.employeeId"
                  :options="empOptions"
                  :placeholder="t('settings.makeupQuotaPickEmployee')"
                  style="width: 200px"
                  show-search
                  option-filter-prop="label"
                />
                <a-input-number v-model:value="overrideForm.quota" :min="0" :max="100" style="width: 120px" />
                <a-button @click="saveMakeupOverride">{{ t('settings.makeupQuotaSave') }}</a-button>
              </a-space>
            </a-form-item>
            <div v-if="makeupOverrides.length" class="small muted">
              <span v-for="o in makeupOverrides" :key="o.employee_id" style="margin-right: 14px">
                {{ empOptions.find((e) => e.value === o.employee_id)?.label || o.employee_id }}: {{ o.makeup_quota }}
              </span>
            </div>
          </a-form>
        </a-card>

        <a-card v-if="isAdmin" :title="t('settings.policyTitle')" class="mb16">
          <template #extra><a-tag color="blue">{{ t('settings.adminOnly') }}</a-tag></template>
          <a-form layout="vertical" style="max-width: 520px">
            <a-space wrap>
              <a-form-item :label="t('settings.workStart')">
                <a-input v-model:value="policyForm.work_start" placeholder="09:00" style="width: 100px" />
              </a-form-item>
              <a-form-item :label="t('settings.workEnd')">
                <a-input v-model:value="policyForm.work_end" placeholder="18:00" style="width: 100px" />
              </a-form-item>
              <a-form-item :label="t('settings.lunchMinutes')">
                <a-input-number v-model:value="policyForm.lunch_minutes" :min="0" :max="240" style="width: 120px" />
              </a-form-item>
              <a-form-item>
                <a-button type="primary" :loading="policySaving" @click="savePolicy">
                  {{ t('settings.policySave') }}
                </a-button>
              </a-form-item>
            </a-space>
          </a-form>

          <a-divider style="margin: 12px 0" />

          <div class="small muted mb8">{{ t('settings.exemptTitle') }}</div>
          <a-space wrap class="mb8">
            <a-radio-group v-model:value="exemptScope">
              <a-radio value="employee">{{ t('settings.exemptEmployee') }}</a-radio>
              <a-radio value="department">{{ t('settings.exemptDepartment') }}</a-radio>
            </a-radio-group>
            <a-select
              v-model:value="exemptTarget"
              :options="exemptScope === 'employee' ? empOptions : deptOptions"
              :placeholder="t('settings.exemptPickTarget')"
              style="width: 220px"
              show-search
              option-filter-prop="label"
            />
            <a-button @click="addExemption">{{ t('settings.exemptAdd') }}</a-button>
          </a-space>
          <div v-if="exemptions.length">
            <a-tag
              v-for="x in exemptions"
              :key="x.scope + x.targetId"
              closable
              style="margin-bottom: 6px"
              @close="removeExemption(x)"
            >
              {{ x.scope === 'department' ? t('settings.exemptDepartment') : t('settings.exemptEmployee') }} ·
              {{ x.scope === 'employee'
                ? (empOptions.find((e) => e.value === x.targetId)?.label || x.targetId)
                : (deptOptions.find((d) => d.value === x.targetId)?.label || x.targetId) }}
            </a-tag>
          </div>
          <a-empty v-else :description="t('settings.exemptEmpty')" :image="false" />
        </a-card>

        <a-card :title="t('settings.endpoint')">
          <div class="desc-grid">
            <div class="desc-item"><span class="desc-k">{{ t('auth.lanAddr') }}</span><span class="desc-v">{{ endpoint.lan || '—' }}</span></div>
            <div class="desc-item"><span class="desc-k">{{ t('auth.wanAddr') }}</span><span class="desc-v">{{ endpoint.wan || '—' }}</span></div>
            <div class="desc-item"><span class="desc-k">{{ t('header.apiMode') }}</span><span class="desc-v">{{ endpoint.mode.toUpperCase() }} · {{ endpoint.base }}</span></div>
          </div>
          <p class="small muted mt8">{{ t('settings.endpointHint') }}</p>
        </a-card>
      </div>
    </div>

    <a-card v-if="isAdmin" :title="t('custom.title')" class="mt16">
      <template #extra>
        <a-button size="small" type="primary" @click="openFieldForm()">
          <Icon name="plus" :size="14" /> {{ t('custom.addField') }}
        </a-button>
      </template>
      <p class="small muted">{{ t('custom.desc') }}</p>
      <a-table
        class="oa-table"
        :columns="[
          { title: t('custom.colId'), dataIndex: 'id', key: 'id' },
          { title: t('custom.colEntity'), key: 'entityType' },
          { title: t('custom.colLabel'), key: 'label' },
          { title: t('custom.colType'), key: 'fieldType' },
          { title: t('custom.colRequired'), key: 'required' },
          { title: t('common.action'), key: 'action' },
        ]"
        :data-source="allFields"
        row-key="id"
        size="small"
        :pagination="false"
      >
        <template #bodyCell="{ column, record }">
          <template v-if="column.key === 'entityType'">
            <a-tag>{{ t(`custom.entity.${record.entityType}`) }}</a-tag>
          </template>
          <template v-else-if="column.key === 'label'">{{ record.label }}</template>
          <template v-else-if="column.key === 'fieldType'">
            {{ t(`custom.type.${record.fieldType}`) }}
            <span v-if="record.fieldType === 'select' && record.options.length" class="small muted">
              （{{ record.options.join(' / ') }}）
            </span>
          </template>
          <template v-else-if="column.key === 'required'">
            <a-tag :color="record.required ? 'red' : 'default'">
              {{ record.required ? t('custom.yes') : t('custom.no') }}
            </a-tag>
          </template>
          <template v-else-if="column.key === 'action'">
            <a-space>
              <a-button type="link" size="small" @click="openFieldForm(record)">{{ t('common.edit') }}</a-button>
              <a-button type="link" size="small" danger @click="removeField(record)">{{ t('custom.delete') }}</a-button>
            </a-space>
          </template>
        </template>
        <template #emptyText><a-empty :description="t('custom.empty')" :image="false" /></template>
      </a-table>
    </a-card>

    <a-modal
      v-model:open="fieldModalOpen"
      :title="fieldEditing ? t('custom.editField') : t('custom.addField')"
      :ok-text="t('custom.save')"
      :cancel-text="t('custom.cancel')"
      @ok="saveField"
      :confirm-loading="false"
    >
      <a-form layout="vertical">
        <a-form-item :label="t('custom.colId')" :extra="t('custom.idHint')">
          <a-input v-model:value="fieldForm.id" :disabled="fieldEditing" placeholder="cf_emergency_contact" />
        </a-form-item>
        <a-form-item :label="t('custom.colEntity')">
          <a-select v-model:value="fieldForm.entityType">
            <a-select-option v-for="et in ENTITY_TYPES" :key="et" :value="et">
              {{ t(`custom.entity.${et}`) }}
            </a-select-option>
          </a-select>
        </a-form-item>
        <a-form-item :label="t('custom.labelZh')">
          <a-input v-model:value="fieldForm.labelZh" />
        </a-form-item>
        <a-form-item :label="t('custom.labelEn')">
          <a-input v-model:value="fieldForm.labelEn" />
        </a-form-item>
        <a-form-item :label="t('custom.colType')">
          <a-select v-model:value="fieldForm.fieldType">
            <a-select-option v-for="ft in FIELD_TYPES" :key="ft" :value="ft">
              {{ t(`custom.type.${ft}`) }}
            </a-select-option>
          </a-select>
        </a-form-item>
        <a-form-item v-if="fieldForm.fieldType === 'select'" :label="t('custom.options')" :extra="t('custom.optionsHint')">
          <a-input v-model:value="fieldForm.optionsText" placeholder="选项A, 选项B" />
        </a-form-item>
        <a-form-item :label="t('custom.colRequired')">
          <a-switch v-model:checked="fieldForm.required" />
        </a-form-item>
        <a-form-item :label="t('custom.sortOrder')">
          <a-input-number v-model:value="fieldForm.sortOrder" :min="0" style="width: 100%" />
        </a-form-item>
      </a-form>
    </a-modal>

    <LlmSettingsModal v-model:open="llmOpen" />
  </div>
</template>
