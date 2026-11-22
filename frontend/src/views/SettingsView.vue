<script setup lang="ts">
/** 设置：账户（改密）/ 模型（管理员）/ 自定义字段库（管理员）/ 服务端地址（只读） */
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { message, Modal } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import LlmSettingsModal from '../components/LlmSettingsModal.vue'
import { callTool, changePassword } from '../api/bridge'
import { useCustomFields } from '../composables/useCustomFields'
import type { CustomEntityType, CustomFieldType } from '../model'
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

onMounted(() => {
  if (isAdmin.value) void reloadFields()
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
