<script setup lang="ts">
/**
 * 模型设置（仅管理员可见）：DeepSeek / Qwen / vLLM / Ollama / OpenAI 都是
 * OpenAI 兼容端点，因此表单只有「提供商 + Base URL + 模型 + API Key」四项。
 *
 * 安全约定：后端只回 Key 的掩码，所以这里永远不回填明文；留空提交表示沿用已存 Key。
 */
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from './Icon.vue'
import {
  getLlmSettings, listLlmModels, saveLlmSettings, testLlmSettings, type LlmSettingsView,
} from '../api/bridge'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const { t } = useI18n()
const view = ref<LlmSettingsView | null>(null)
const loading = ref(false)
const saving = ref(false)
const testing = ref(false)
const modelsLoading = ref(false)
const testResult = ref('')
/** 从 `GET {base_url}/models` 拉到的可用模型，供模型名下拉 */
const models = ref<string[]>([])
const form = reactive({ provider: '', base_url: '', model: '', api_key: '' })

const presets = computed(() => view.value?.providers ?? [])
const needsKey = computed(
  () => presets.value.find((p) => p.code === form.provider)?.needs_key ?? true,
)
const keyPlaceholder = computed(() =>
  view.value?.has_key ? view.value.key_hint : t('llm.keyEmpty'),
)
/** 下拉选项：接口返回的模型清单；当前值不在清单里时补进去，避免选中项显示为空 */
const modelOptions = computed(() => {
  const list = models.value.slice()
  if (form.model && !list.includes(form.model)) list.unshift(form.model)
  return list.map((m) => ({ label: m, value: m }))
})
function filterModel(input: string, option?: { value?: string }): boolean {
  const text = String(input || '').toLowerCase()
  const v = String(option?.value ?? '').toLowerCase()
  // 输入框里就是当前已选模型名时不做过滤：否则展开下拉只会看到自己那一项，没法换
  if (text && text === (form.model || '').toLowerCase()) return true
  return v.includes(text)
}
const statusText = computed(() => {
  if (!view.value) return ''
  if (view.value.source === 'db') {
    return t('llm.statusDb', { provider: view.value.provider, model: view.value.model })
  }
  if (view.value.source === 'env') return t('llm.statusEnv')
  return t('llm.statusNone')
})

function fill(next: LlmSettingsView): void {
  view.value = next
  const matched = next.providers.find((p) => p.code === next.provider) ?? next.providers[0]
  form.provider = matched?.code ?? ''
  form.base_url = next.base_url || matched?.base_url || ''
  form.model = next.model || matched?.model || ''
  form.api_key = ''
  // 换了配置就作废旧的模型清单，下次点「测试连接」按新 Base URL 重新拉
  models.value = []
}

async function load(): Promise<void> {
  loading.value = true
  testResult.value = ''
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

/** 切换提供商时带出预设的 Base URL 与模型，仍可手改（自建端点、私有模型名） */
function applyPreset(code: string): void {
  form.provider = code
  const p = presets.value.find((x) => x.code === code)
  if (p) {
    form.base_url = p.base_url
    form.model = p.model
  }
}

async function onSave(): Promise<void> {
  saving.value = true
  try {
    const next = await saveLlmSettings({
      provider: form.provider,
      base_url: form.base_url,
      model: form.model,
      api_key: form.api_key,
    })
    fill(next)
    testResult.value = ''
    message.success(t('llm.saved'))
    emit('update:open', false)
  } catch (e) {
    message.error(String(e))
  } finally {
    saving.value = false
  }
}

/**
 * 按当前 Base URL 拉一次模型清单。
 * 返回是否成功——失败不阻断后续连通性测试（有些兼容端点不实现 /models，但 chat 可用）。
 */
async function loadModels(): Promise<boolean> {
  if (!form.base_url.trim()) {
    message.warning(t('llm.baseUrlNeeded'))
    return false
  }
  modelsLoading.value = true
  try {
    const list = await listLlmModels(form.base_url, form.api_key)
    models.value = list
    if (!list.length) {
      message.warning(t('llm.modelsEmpty'))
    } else if (!list.includes(form.model)) {
      // 当前模型名不在清单里（换端点/空值）：默认选第一个，省得对着空下拉发呆
      form.model = list[0]
    }
    return true
  } catch (e) {
    message.warning(`${t('llm.modelsFailed')}: ${String(e)}`)
    return false
  } finally {
    modelsLoading.value = false
  }
}

async function onTest(): Promise<void> {
  testing.value = true
  testResult.value = ''
  try {
    // 先按 Base URL 刷新模型下拉，再用选中的模型发一次最小对话
    await loadModels()
    const res = await testLlmSettings({
      base_url: form.base_url,
      model: form.model,
      api_key: form.api_key,
    })
    testResult.value = res.reply
    message.success(t('llm.testOk'))
  } catch (e) {
    message.error(String(e))
  } finally {
    testing.value = false
  }
}

async function onClear(): Promise<void> {
  saving.value = true
  try {
    fill(await saveLlmSettings({
      provider: '', base_url: '', model: '', api_key: '', clear: true,
    }))
    message.success(t('llm.cleared'))
  } catch (e) {
    message.error(String(e))
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <a-modal
    :open="props.open" :title="t('llm.title')" width="620px"
    :ok-text="t('llm.save')" :cancel-text="t('llm.cancel')" :confirm-loading="saving"
    @ok="onSave" @update:open="(v: boolean) => emit('update:open', v)"
  >
    <a-spin :spinning="loading">
      <a-form layout="vertical">
        <a-form-item :label="t('llm.provider')">
          <a-select
            v-model:value="form.provider"
            :placeholder="t('llm.providerPlaceholder')"
            @change="applyPreset"
          >
            <a-select-option v-for="p in presets" :key="p.code" :value="p.code">
              {{ p.label }}
            </a-select-option>
          </a-select>
        </a-form-item>
        <a-form-item :label="t('llm.baseUrl')" :extra="t('llm.baseUrlHint')">
          <a-input v-model:value="form.base_url" placeholder="https://api.deepseek.com/v1" />
        </a-form-item>
        <a-form-item :label="t('llm.model')" :extra="t('llm.modelHint')">
          <!-- 下拉选可用模型，同时允许直接输入私有模型名（自建端点常见） -->
          <a-auto-complete
            v-model:value="form.model"
            :options="modelOptions"
            :filter-option="filterModel"
            :placeholder="t('llm.modelPlaceholder')"
            style="width: 100%"
          >
            <template #notFoundContent>
              <a-spin v-if="modelsLoading" size="small" />
              <span v-else class="small muted">{{ t('llm.modelsHint') }}</span>
            </template>
          </a-auto-complete>
        </a-form-item>
        <a-form-item :label="t('llm.apiKey')" :extra="needsKey ? t('llm.keyHint') : t('llm.keyOptional')">
          <a-input-password
            v-model:value="form.api_key" :placeholder="keyPlaceholder"
            autocomplete="new-password"
          />
        </a-form-item>
      </a-form>

      <a-alert
        :type="view?.mode === 'llm' ? 'success' : 'warning'" show-icon class="mb16"
        :message="statusText"
      />

      <div class="llm-actions">
        <a-button :loading="testing" @click="onTest">
          <Icon name="zap" :size="13" /> {{ testing ? t('llm.testing') : t('llm.test') }}
        </a-button>
        <a-button type="text" danger :disabled="view?.source !== 'db'" @click="onClear">
          <Icon name="refresh" :size="13" /> {{ t('llm.clear') }}
        </a-button>
        <span class="spacer"></span>
        <span v-if="testResult" class="llm-reply">{{ t('llm.testReply') }}: {{ testResult }}</span>
      </div>
    </a-spin>
  </a-modal>
</template>

<style scoped>
.llm-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.llm-reply {
  font-size: 12px;
  color: var(--oa-muted);
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
