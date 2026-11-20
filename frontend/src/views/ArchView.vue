<script setup lang="ts">
/** 系统架构：技术栈 / 分层架构图 / 20 工具协议 / 五端打包矩阵 / 安全治理 */
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import {
  ARCH_LAYERS, PLATFORMS, SECURITY_KEYS, STACK_CARDS, TOOL_CATALOG,
} from '../model'
import { L } from '../utils'
import { loadTools, session } from '../store'

const { t } = useI18n()
onMounted(() => { if (!session.tools.length) void loadTools() })

interface ToolRow { name: string; domain: string; desc: string; params: string; allowed: boolean }
const toolRows = computed<ToolRow[]>(() => {
  if (session.tools.length) {
    return session.tools.map((s) => {
      const schema = s.parameters as { properties?: Record<string, unknown> } | undefined
      const params = schema?.properties ? Object.keys(schema.properties).join(', ') : String(s.params || '')
      return {
        name: s.name,
        domain: String(s.domain || 'system'),
        desc: L(s.description || s.desc),
        params,
        allowed: s.allowed !== false,
      }
    })
  }
  return TOOL_CATALOG.map((x) => ({
    name: x.name, domain: x.domain, desc: L(x.desc), params: x.params, allowed: true,
  }))
})
const toolColumns = computed(() => [
  { title: t('arch.toolCol'), dataIndex: 'name', key: 'name' },
  { title: t('arch.toolDomain'), key: 'domain' },
  { title: t('arch.toolDesc'), dataIndex: 'desc', key: 'desc' },
  { title: t('arch.toolParams'), dataIndex: 'params', key: 'params' },
])
const platformColumns = computed(() => [
  { title: t('arch.platformCol'), key: 'platform' },
  { title: t('arch.platformCmd'), key: 'cmd' },
])
const noteKeys: Record<string, string> = { win: 'pWin', mac: 'pMac', linux: 'pLinux', android: 'pAndroid', ios: 'pIos' }
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('arch.title') }}</h1><p>{{ t('arch.desc') }}</p></div>
    </div>

    <a-alert type="info" show-icon class="mb16" :message="t('arch.disclaimer')" />

    <!-- 技术栈 -->
    <a-card :title="t('arch.stackTitle')" class="mb16">
      <template #extra><span class="small muted">Vue 3 · Rust · Rig · Tauri 2.0</span></template>
      <div class="oa-grid g-4">
        <div class="stack-card" v-for="c in STACK_CARDS" :key="c.key">
          <div class="sc-ico" :class="'bg-' + c.color"><Icon :name="c.ico" :size="18" /></div>
          <h4>{{ c.title }}</h4>
          <p>{{ t('arch.stack.' + c.key) }}</p>
        </div>
      </div>
    </a-card>

    <!-- 分层架构 + 工具协议 -->
    <div class="oa-grid g-1-2 mb16">
      <a-card :title="t('arch.archTitle')">
        <div class="arch-flow">
          <template v-for="(l, i) in ARCH_LAYERS" :key="l.k">
            <div class="arch-layer" :class="l.k === 'l1' ? '' : l.k === 'l2' ? 'sec' : l.k === 'l3' ? 'third' : 'fourth'">
              <h4><Icon :name="l.ico" :size="15" /> {{ t('arch.' + l.k) }}</h4>
              <p>{{ t('arch.' + l.k + 'd') }}</p>
              <div class="arch-pills"><span class="arch-pill" v-for="p in l.pills" :key="p">{{ p }}</span></div>
            </div>
            <div class="arch-arrow" v-if="i < ARCH_LAYERS.length - 1">↓</div>
          </template>
        </div>
      </a-card>

      <a-card :title="t('arch.toolsTitle')">
        <template #extra><span class="small muted">{{ t('arch.toolsSub') }}</span></template>
        <a-table class="oa-table" :columns="toolColumns" :data-source="toolRows" row-key="name" :pagination="false" size="small">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">
              <code class="tool-name">{{ record.name }}</code>
              <a-tag v-if="!record.allowed" color="default" style="margin-left: 6px">{{ t('arch.noPerm') }}</a-tag>
            </template>
            <template v-else-if="column.key === 'domain'"><a-tag color="blue">{{ t('module.' + record.domain) }}</a-tag></template>
            <template v-else-if="column.key === 'desc'"><span class="small">{{ record.desc }}</span></template>
            <template v-else-if="column.key === 'params'"><span class="mono small muted">{{ record.params }}</span></template>
          </template>
        </a-table>
      </a-card>
    </div>

    <!-- 打包矩阵 + 安全 -->
    <div class="oa-grid g-1-2">
      <a-card :title="t('arch.platformTitle')">
        <template #extra><span class="small muted">Tauri 2.0</span></template>
        <a-table class="oa-table" :columns="platformColumns" :data-source="PLATFORMS" row-key="key" :pagination="false" size="small">
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'platform'">
              <div style="display: flex; align-items: center; gap: 8px">
                <span class="stat-ico" :class="'bg-' + record.color" style="width: 28px; height: 28px"><Icon :name="record.icon" :size="15" /></span>
                <b>{{ t('arch.' + record.key) }}</b>
              </div>
            </template>
            <template v-else-if="column.key === 'cmd'">
              <div class="mono small">{{ record.cmd }}</div>
              <div class="small muted" style="margin-top: 3px">{{ t('arch.' + noteKeys[record.key]) }}</div>
            </template>
          </template>
        </a-table>
      </a-card>

      <a-card :title="t('arch.securityTitle')">
        <a-alert
          v-for="(k, i) in SECURITY_KEYS" :key="k" type="info" show-icon class="mb16"
          :message="t('arch.' + k)"
        >
          <template #icon><Icon :name="['key', 'shield', 'users', 'activity', 'refresh'][i]" :size="15" /></template>
        </a-alert>
      </a-card>
    </div>
  </div>
</template>
