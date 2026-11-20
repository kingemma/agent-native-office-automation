<script setup lang="ts">
/** 表格「列设置」下拉：勾选显隐 + 上下移排序，固定列不可隐藏 */
import { useI18n } from 'vue-i18n'
import Icon from './Icon.vue'
import type { ColumnMeta } from '../composables/useColumns'

const { t } = useI18n()

defineProps<{
  model: {
    ordered: ColumnMeta[]
    isHidden: (key: string) => boolean
    toggle: (key: string) => void
    move: (key: string, dir: -1 | 1) => void
    reset: () => void
  }
}>()
</script>

<template>
  <a-popover trigger="click" placement="bottomRight" :title="t('columns.title')">
    <template #content>
      <div class="col-picker">
        <div class="col-picker-item" v-for="c in model.ordered" :key="c.key">
          <a-checkbox
            :checked="!model.isHidden(c.key)"
            :disabled="c.locked"
            @change="model.toggle(c.key)"
          >{{ c.title }}</a-checkbox>
          <span class="spacer"></span>
          <span class="col-picker-move">
            <a-button size="small" type="text" @click="model.move(c.key, -1)"><Icon name="up" :size="11" /></a-button>
            <a-button size="small" type="text" @click="model.move(c.key, 1)"><Icon name="down" :size="11" /></a-button>
          </span>
        </div>
        <a-button size="small" block style="margin-top: 8px" @click="model.reset">{{ t('columns.reset') }}</a-button>
      </div>
    </template>
    <a-button size="small"><Icon name="filter" :size="14" /> {{ t('columns.label') }}</a-button>
  </a-popover>
</template>
