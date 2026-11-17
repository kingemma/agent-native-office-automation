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
  getLlmSettings, saveLlmSettings, testLlmSettings, type LlmSettingsView,
} from '../api/bridge'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const { t } = useI18n()
const view = ref<LlmSettingsView | null>(null)
const loading = ref(false)
const saving = ref(false)
const testing = ref(false)
const testResult = ref('')
const form = reactive({ provider: '', base_url: '', model: '', api_key: '' })

const presets = computed(() => view.value?.providers ?? [])
const needsKey = computed(
  () => presets.value.find((p) => p.code === form.provider)?.needs_key ?? true,
)
const keyPlaceholder = computed(() =>
  view.value?.has_key ? view.value.key_hint : t('llm.keyEmpty'),
)
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

async function onTest(): Promise<void> {
  testing.value = true
  testResult.value = ''
  try {
    const res = await testLlmSettings()
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
          <a-input v-model:value="form.model" placeholder="deepseek-chat" />
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
