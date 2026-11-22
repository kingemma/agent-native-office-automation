/** 自定义字段库：加载字段定义、读取记录字段值、组装创建表单提交参数。 */
import { computed, ref } from 'vue'
import { listTable } from '../api/bridge'
import {
  toCustomField,
  toCustomFieldValue,
  type CustomEntityType,
  type CustomField,
  type CustomFieldValue,
} from '../model'

/** 单个 composable 实例共享一份缓存，避免四个视图重复拉取同一张表 */
const fieldsCache = ref<CustomField[]>([])
const valuesCache = ref<CustomFieldValue[]>([])
let loadedAt = 0

export function useCustomFields() {
  const loading = ref(false)

  async function reload(): Promise<void> {
    loading.value = true
    try {
      const [fieldRows, valueRows] = await Promise.all([
        listTable<Record<string, unknown>>('custom_fields'),
        listTable<Record<string, unknown>>('custom_field_values'),
      ])
      fieldsCache.value = fieldRows.map(toCustomField)
      valuesCache.value = valueRows.map(toCustomFieldValue)
      loadedAt = Date.now()
    } finally {
      loading.value = false
    }
  }

  async function ensureLoaded(): Promise<void> {
    // 30 秒内的缓存直接复用；字段变更后调用 reload() 立即刷新
    if (loadedAt && Date.now() - loadedAt < 30_000) return
    await reload()
  }

  function fieldsFor(entityType: CustomEntityType): CustomField[] {
    return fieldsCache.value
      .filter((f) => f.entityType === entityType && f.enabled)
      .sort((a, b) => a.sortOrder - b.sortOrder || a.id.localeCompare(b.id))
  }

  function valuesFor(entityType: CustomEntityType, entityId: string): CustomFieldValue[] {
    return valuesCache.value.filter((v) => v.entityType === entityType && v.entityId === entityId)
  }

  /** 创建表单初值：字段名用 field_id，便于原样作为 custom_fields 提交 */
  function emptyValues(entityType: CustomEntityType): Record<string, string> {
    const out: Record<string, string> = {}
    for (const f of fieldsFor(entityType)) out[f.id] = ''
    return out
  }

  /** 过滤空值与字段库中不存在的键，只提交有内容的字段 */
  function submissionValues(
    entityType: CustomEntityType,
    draft: Record<string, string>,
  ): Record<string, string> {
    const allowed = new Set(fieldsFor(entityType).map((f) => f.id))
    const out: Record<string, string> = {}
    for (const [key, value] of Object.entries(draft)) {
      if (allowed.has(key) && String(value ?? '').trim()) out[key] = String(value)
    }
    return out
  }

  const allFields = computed(() => fieldsCache.value)

  return {
    loading,
    allFields,
    reload,
    ensureLoaded,
    fieldsFor,
    valuesFor,
    emptyValues,
    submissionValues,
  }
}
