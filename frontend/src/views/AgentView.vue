<script setup lang="ts">
/** 全屏 Agent 工作台页（与侧边抽屉共享 store 会话状态） */
import AgentWorkbench from '../components/AgentWorkbench.vue'
</script>

<template>
  <AgentWorkbench :compact="false" />
</template>
