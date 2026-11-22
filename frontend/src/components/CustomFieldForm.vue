/** 自定义字段动态表单：按实体类型渲染字段库中的字段，并做必填校验。 */
<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useCustomFields } from '../composables/useCustomFields'
import type { CustomEntityType } from '../model'

const props = defineProps<{ entityType: CustomEntityType; values: Record<string, string> }>()
const emit = defineEmits<{ 'update:values': [value: Record<string, string>] }>()
const { t } = useI18n()
const { fieldsFor } = useCustomFields()

const fields = computed(() => fieldsFor(props.entityType))

function setField(id: string, value: unknown): void {
  emit('update:values', { ...props.values, [id]: value == null ? '' : String(value) })
}

function validate(): boolean {
  return fields.value.every((f) => !f.required || (props.values[f.id] ?? '').trim())
}

defineExpose({ validate })
</script>

<template>
  <div v-if="fields.length" class="custom-fields">
    <div class="custom-fields-title">{{ t('custom.formTitle') }}</div>
    <div class="oa-form-grid">
      <a-form-item v-for="f in fields" :key="f.id" :label="f.label" :required="f.required">
        <a-input
          v-if="f.fieldType === 'text'"
          :value="values[f.id] ?? ''"
          @update:value="setField(f.id, $event)"
        />
        <a-input-number
          v-else-if="f.fieldType === 'number'"
          :value="values[f.id] ? Number(values[f.id]) : undefined"
          @update:value="setField(f.id, $event)"
          style="width: 100%"
        />
        <a-date-picker
          v-else-if="f.fieldType === 'date'"
          :value="values[f.id] || undefined"
          value-format="YYYY-MM-DD"
          @update:value="setField(f.id, $event)"
          style="width: 100%"
        />
        <a-select
          v-else-if="f.fieldType === 'select'"
          :value="values[f.id] || undefined"
          @update:value="setField(f.id, $event)"
          allow-clear
        >
          <a-select-option v-for="opt in f.options" :key="opt" :value="opt">{{ opt }}</a-select-option>
        </a-select>
      </a-form-item>
    </div>
  </div>
</template>

<style scoped>
.custom-fields { margin-top: 8px; padding-top: 8px; border-top: 1px dashed var(--border, #e5e7eb); }
.custom-fields-title { font-size: 13px; font-weight: 600; margin-bottom: 8px; color: var(--text-muted, #6b7280); }
</style>
