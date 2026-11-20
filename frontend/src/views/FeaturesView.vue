<script setup lang="ts">
/** 功能验收矩阵：直接渲染 /api/features（与后端 domain::FEATURES 同一份常量） */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import AsyncPanel from '../components/AsyncPanel.vue'
import { getFeatures, type FeatureItem } from '../api/bridge'

const { t } = useI18n()
const loading = ref(false)
const error = ref('')
const features = ref<FeatureItem[]>([])

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    features.value = await getFeatures()
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}
onMounted(load)

const DOMAIN_LABEL: Record<string, string> = {
  hr: '人事', org: '组织', finance: '财务', contract: '合同',
  attendance: '考勤', asset: '资产', agent: 'Agent', platform: '平台',
}
const groups = computed(() => {
  const map = new Map<string, FeatureItem[]>()
  for (const f of features.value) {
    if (!map.has(f.domain)) map.set(f.domain, [])
    map.get(f.domain)!.push(f)
  }
  return Array.from(map.entries()).map(([domain, items]) => ({
    domain,
    label: DOMAIN_LABEL[domain] || domain,
    items,
  }))
})
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div>
        <h1>{{ t('features.title') }}</h1>
        <p>{{ t('features.desc') }}</p>
      </div>
      <span class="spacer"></span>
      <a-tag color="blue">{{ t('features.total', { n: features.length }) }}</a-tag>
    </div>

    <AsyncPanel :loading="loading" :error="error" @retry="load">
      <a-card v-for="g in groups" :key="g.domain" class="mb16">
        <template #title>
          <span class="feature-domain">{{ g.label }}</span>
        </template>
        <div class="feature-list">
          <div class="feature-row" v-for="it in g.items" :key="it.id">
            <code class="tool-name">{{ it.id }}</code>
            <span class="feature-text">{{ it.text }}</span>
            <span class="spacer"></span>
            <a-tag color="green"><Icon name="checked" :size="12" /> {{ t('features.done') }}</a-tag>
          </div>
        </div>
      </a-card>
    </AsyncPanel>
  </div>
</template>
