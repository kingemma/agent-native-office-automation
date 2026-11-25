<script setup lang="ts">
/**
 * 模型设置（仅管理员）：维护一份模型目录，可添加多条
 * （显示名称 + 提供商 + Base URL + 模型 + Key），并指定其中一条为默认。
 *
 * 对话页下拉展示的是「显示名称」，回传给服务端的也是名称——服务端按名称取该条目的
 * 整份配置（端点与 Key 都跟着条目走），所以同一端点可以配多个 Key、不同端点可以配同名模型。
 *
 * 安全约定：后端只回 Key 的掩码，这里永远不回填明文；某条留空提交表示沿用同名旧 Key。
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from './Icon.vue'
import {
  getLlmSettings, listLlmModels, saveLlmCatalog, testLlmSettings,
  type LlmCatalogModel, type LlmSettingsView,
} from '../api/bridge'
import { reloadAgentModels } from '../store'

/** 编辑中的一行：`models` 是该行自己拉到的候选，互不干扰 */
interface Row {
  name: string
  provider: string
  base_url: string
  model: string
  api_key: string
  hasKey: boolean
  keyHint: string
  models: string[]
  modelsLoading: boolean
  testing: boolean
  testResult: string
}

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const { t } = useI18n()
const view = ref<LlmSettingsView | null>(null)
const loading = ref(false)
const saving = ref(false)
const rows = ref<Row[]>([])
/** 默认模型用下标记录：改名字不会让「默认」指向失效的名称 */
const defaultIdx = ref(0)

const presets = computed(() => view.value?.providers ?? [])

function toRow(m: LlmCatalogModel): Row {
  return {
    name: m.name,
    provider: m.provider,
    base_url: m.base_url,
    model: m.model,
    api_key: '',
    hasKey: m.has_key,
    keyHint: m.key_hint,
    models: [],
    modelsLoading: false,
    testing: false,
    testResult: '',
  }
}

function emptyRow(): Row {
  const p = presets.value[0]
  return {
    name: '',
    provider: p?.code ?? '',
    base_url: p?.base_url ?? '',
    model: p?.model ?? '',
    api_key: '',
    hasKey: false,
    keyHint: '',
    models: [],
    modelsLoading: false,
    testing: false,
    testResult: '',
  }
}

function needsKey(row: Row): boolean {
  return presets.value.find((p) => p.code === row.provider)?.needs_key ?? true
}

function keyPlaceholder(row: Row): string {
  return row.hasKey ? row.keyHint : t('llm.keyEmpty')
}

/** 该行的模型候选：接口返回的清单；当前值不在清单里时补进去，避免显示为空 */
function modelOptions(row: Row): Array<{ label: string; value: string }> {
  const list = row.models.slice()
  if (row.model && !list.includes(row.model)) list.unshift(row.model)
  return list.map((m) => ({ label: m, value: m }))
}

function fill(next: LlmSettingsView): void {
  view.value = next
  rows.value = next.models.map(toRow)
  const at = rows.value.findIndex((r) => r.name === next.default_model)
  defaultIdx.value = at >= 0 ? at : 0
}

async function load(): Promise<void> {
  loading.value = true
  try {
    fill(await getLlmSettings())
  } catch (e) {
    message.error(String(e))
  } finally {
    loading.value = false
  }
}

watch(
  () => props.open,
  (open) => {
    if (open) void load()
  },
)

function addRow(): void {
  rows.value.push(emptyRow())
}

function removeRow(i: number): void {
  rows.value.splice(i, 1)
  // 删掉的正好是默认行（或删在它前面）：默认下移到第一条，避免指向不存在的下标
  if (defaultIdx.value >= rows.value.length) defaultIdx.value = 0
  else if (i < defaultIdx.value) defaultIdx.value -= 1
}

/** 切换提供商时带出预设的 Base URL 与模型，仍可手改（自建端点、私有模型名） */
function applyPreset(row: Row, code: string): void {
  row.provider = code
  const p = presets.value.find((x) => x.code === code)
  if (p) {
    row.base_url = p.base_url
    row.model = p.model
  }
}

/** 按该行 Base URL 拉一次模型候选；失败不阻断连通性测试 */
async function fetchModels(row: Row): Promise<boolean> {
  if (!row.base_url.trim()) {
    message.warning(t('llm.baseUrlNeeded'))
    return false
  }
  row.modelsLoading = true
  try {
    const list = await listLlmModels(row.base_url, row.api_key)
    row.models = list
    if (!list.length) {
      message.warning(t('llm.modelsEmpty'))
    } else if (!list.includes(row.model)) {
      row.model = list[0]
    }
    return true
  } catch (e) {
    message.warning(`${t('llm.modelsFailed')}: ${String(e)}`)
    return false
  } finally {
    row.modelsLoading = false
  }
}

async function onTest(row: Row): Promise<void> {
  row.testing = true
  row.testResult = ''
  try {
    await fetchModels(row)
    const res = await testLlmSettings({
      base_url: row.base_url,
      model: row.model,
      api_key: row.api_key,
    })
    row.testResult = res.reply
    message.success(t('llm.testOk'))
  } catch (e) {
    message.error(String(e))
  } finally {
    row.testing = false
  }
}

/** 校验只做界面能给出好提示的部分，Key/端点合法性交给服务端统一判定 */
function validate(): boolean {
  const seen = new Set<string>()
  for (const row of rows.value) {
    const name = row.name.trim()
    if (!name) {
      message.error(t('llm.nameRequired'))
      return false
    }
    const lower = name.toLowerCase()
    if (seen.has(lower)) {
      message.error(t('llm.nameDuplicate', { name }))
      return false
    }
    seen.add(lower)
  }
  if (rows.value.length && !rows.value[defaultIdx.value]) {
    message.error(t('llm.defaultMissing'))
    return false
  }
  return true
}

async function onSave(): Promise<void> {
  if (!validate()) return
  saving.value = true
  try {
    const next = await saveLlmCatalog({
      default: rows.value.length ? rows.value[defaultIdx.value].name.trim() : '',
      models: rows.value.map((row) => ({
        name: row.name.trim(),
        provider: row.provider,
        base_url: row.base_url.trim(),
        model: row.model.trim(),
        api_key: row.api_key,
      })),
    })
    fill(next)
    message.success(t('llm.saved'))
    // 对话页的模型下拉可能已经过期（新增/改名/删除了条目），这里直接重拉
    void reloadAgentModels()
    emit('update:open', false)
  } catch (e) {
    message.error(String(e))
  } finally {
    saving.value = false
  }
}

/** 清空整张表：服务端会同时删掉单条配置，回落到环境变量 / 离线引擎 */
async function onClear(): Promise<void> {
  saving.value = true
  try {
    fill(await saveLlmCatalog({ default: '', models: [] }))
    message.success(t('llm.cleared'))
    void reloadAgentModels()
  } catch (e) {
    message.error(String(e))
  } finally {
    saving.value = false
  }
}

const statusText = computed(() => {
  const v = view.value
  if (!v) return ''
  if (v.source === 'db') {
    const def = v.models.find((m) => m.name === v.default_model)
    if (def) return t('llm.statusDb', { provider: def.provider, model: def.model })
  }
  if (v.source === 'env') return t('llm.statusEnv')
  return t('llm.statusNone')
})
</script>

<template>
  <a-modal
    :open="props.open" :title="t('llm.title')" width="780px"
    :ok-text="t('llm.save')" :cancel-text="t('llm.cancel')" :confirm-loading="saving"
    @ok="onSave" @update:open="(v: boolean) => emit('update:open', v)"
  >
    <a-spin :spinning="loading">
      <a-alert
        :type="view?.mode === 'llm' ? 'success' : 'warning'" show-icon class="mb16"
        :message="statusText" :description="t('llm.catalogHint')"
      />

      <a-alert
        v-if="view?.source === 'env'" type="warning" show-icon class="mb16"
        :message="t('llm.envOnly')"
      />

      <a-empty v-if="!rows.length" :description="t('llm.catalogEmpty')" class="llm-empty" />

      <a-radio-group v-model:value="defaultIdx" class="llm-rows">
        <div class="llm-row" v-for="(row, i) in rows" :key="i">
          <div class="llm-row-head">
            <a-radio :value="i">{{ t('llm.defaultTag') }}</a-radio>
            <a-input
              v-model:value="row.name" class="llm-row-name"
              :placeholder="t('llm.modelNamePh')" :title="t('llm.modelNameHint')"
            />
            <span class="spacer"></span>
            <a-button type="text" danger size="small" @click="removeRow(i)">
              <Icon name="x" :size="13" /> {{ t('llm.removeModel') }}
            </a-button>
          </div>

          <div class="llm-row-grid">
            <a-form-item :label="t('llm.provider')" class="llm-cell">
              <a-select
                :value="row.provider" :placeholder="t('llm.providerPlaceholder')"
                @change="(v: string) => applyPreset(row, v)"
              >
                <a-select-option v-for="p in presets" :key="p.code" :value="p.code">
                  {{ p.label }}
                </a-select-option>
              </a-select>
            </a-form-item>
            <a-form-item :label="t('llm.baseUrl')" class="llm-cell">
              <a-input v-model:value="row.base_url" placeholder="https://api.deepseek.com/v1" />
            </a-form-item>

            <a-form-item :label="t('llm.model')" class="llm-cell">
              <div class="llm-model-line">
                <a-auto-complete
                  v-model:value="row.model" :options="modelOptions(row)"
                  :placeholder="t('llm.modelPlaceholder')" style="flex: 1"
                >
                  <template #notFoundContent>
                    <a-spin v-if="row.modelsLoading" size="small" />
                    <span v-else class="small muted">{{ t('llm.modelsHint') }}</span>
                  </template>
                </a-auto-complete>
                <a-button size="small" :loading="row.modelsLoading" @click="fetchModels(row)">
                  {{ t('llm.fetchModels') }}
                </a-button>
              </div>
            </a-form-item>

            <a-form-item
              :label="t('llm.apiKey')" class="llm-cell"
              :extra="needsKey(row) ? t('llm.keyHint') : t('llm.keyOptional')"
            >
              <a-input-password
                v-model:value="row.api_key" :placeholder="keyPlaceholder(row)"
                autocomplete="new-password"
              />
            </a-form-item>
          </div>

          <div class="llm-row-actions">
            <a-button size="small" :loading="row.testing" @click="onTest(row)">
              <Icon name="zap" :size="13" /> {{ row.testing ? t('llm.testing') : t('llm.test') }}
            </a-button>
            <span v-if="row.testResult" class="llm-reply">
              {{ t('llm.testReply') }}: {{ row.testResult }}
            </span>
          </div>
        </div>
      </a-radio-group>

      <div class="llm-actions">
        <a-button type="dashed" @click="addRow">
          <Icon name="plus" :size="13" /> {{ t('llm.addModel') }}
        </a-button>
        <a-button type="text" danger :disabled="view?.source !== 'db'" @click="onClear">
          <Icon name="refresh" :size="13" /> {{ t('llm.clearAll') }}
        </a-button>
      </div>
    </a-spin>
  </a-modal>
</template>

<style scoped>
.llm-rows { display: flex; flex-direction: column; gap: 12px; width: 100%; }
.llm-row {
  border: 1px solid var(--oa-border); border-radius: 10px; padding: 12px;
  background: #fbfcfe;
}
.llm-row-head { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
.llm-row-name { max-width: 260px; }
.llm-row-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0 12px; }
.llm-cell { margin-bottom: 8px; }
.llm-model-line { display: flex; align-items: center; gap: 6px; }
.llm-row-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.llm-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 14px; }
.llm-empty { padding: 12px 0 4px; }
.llm-reply {
  font-size: 12px;
  color: var(--oa-muted);
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
@media (max-width: 720px) {
  .llm-row-grid { grid-template-columns: 1fr; }
}
</style>
