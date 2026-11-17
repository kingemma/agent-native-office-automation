<script setup lang="ts">
/** 统一的加载 / 错误 / 空态容器：后端未启动时显示可重试提示，绝不白屏 */
import { useI18n } from 'vue-i18n'

defineProps<{ loading: boolean; error: string; empty?: boolean }>()
const emit = defineEmits<{ retry: [] }>()
const { t } = useI18n()
</script>

<template>
  <div v-if="loading" class="async-state">
    <a-spin />
    <span class="muted">{{ t('common.loading') }}</span>
  </div>
  <div v-else-if="error" class="async-state async-err">
    <a-alert type="error" show-icon :message="t('error.backendDown')" :description="error">
      <template #action>
        <a-button size="small" danger @click="emit('retry')">{{ t('common.retry') }}</a-button>
      </template>
    </a-alert>
  </div>
  <div v-else-if="empty" class="async-state">
    <a-empty :description="t('common.noData')" />
  </div>
  <slot v-else />
</template>
