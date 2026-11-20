<script setup lang="ts">
/** 设置：账户（改密）/ 模型（管理员）/ 服务端地址（只读，登录页可改） */
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from '../components/Icon.vue'
import LlmSettingsModal from '../components/LlmSettingsModal.vue'
import { changePassword } from '../api/bridge'
import { auth, doLogout, endpoint } from '../store'

const { t } = useI18n()
const user = computed(() => auth.user)
const llmOpen = ref(false)

const pwdForm = reactive({ old: '', next: '', confirm: '' })
const pwdLoading = ref(false)

async function submitPassword(): Promise<void> {
  if (pwdForm.next !== pwdForm.confirm) {
    message.warning(t('auth.passwordMismatch'))
    return
  }
  pwdLoading.value = true
  try {
    await changePassword(pwdForm.old, pwdForm.next)
    message.success(t('auth.passwordChanged'))
    await doLogout()
  } catch (e) {
    message.error(String(e))
  } finally {
    pwdLoading.value = false
  }
}
</script>

<template>
  <div>
    <div class="oa-page-head">
      <div><h1>{{ t('settings.title') }}</h1><p>{{ t('settings.desc') }}</p></div>
    </div>

    <div class="oa-grid g-2-1">
      <a-card :title="t('settings.account')">
        <a-form layout="vertical" style="max-width: 360px">
          <a-form-item :label="t('auth.oldPassword')">
            <a-input-password v-model:value="pwdForm.old" autocomplete="current-password" />
          </a-form-item>
          <a-form-item :label="t('auth.newPassword')">
            <a-input-password v-model:value="pwdForm.next" autocomplete="new-password" />
          </a-form-item>
          <a-form-item :label="t('auth.newPassword')">
            <a-input-password v-model:value="pwdForm.confirm" autocomplete="new-password" />
          </a-form-item>
          <a-button type="primary" :loading="pwdLoading" @click="submitPassword">
            {{ t('auth.changePassword') }}
          </a-button>
        </a-form>
      </a-card>

      <div>
        <a-card :title="t('settings.model')" class="mb16">
          <template #extra>
            <a-tag v-if="user?.role === 'admin'" color="blue">{{ t('settings.adminOnly') }}</a-tag>
          </template>
          <p class="small muted">{{ t('settings.modelDesc') }}</p>
          <a-button v-if="user?.role === 'admin'" @click="llmOpen = true">
            <Icon name="settings" :size="14" /> {{ t('llm.entry') }}
          </a-button>
          <a-empty v-else :description="t('settings.modelForbidden')" :image="false" />
        </a-card>

        <a-card :title="t('settings.endpoint')">
          <div class="desc-grid">
            <div class="desc-item"><span class="desc-k">{{ t('auth.lanAddr') }}</span><span class="desc-v">{{ endpoint.lan || '—' }}</span></div>
            <div class="desc-item"><span class="desc-k">{{ t('auth.wanAddr') }}</span><span class="desc-v">{{ endpoint.wan || '—' }}</span></div>
            <div class="desc-item"><span class="desc-k">{{ t('header.apiMode') }}</span><span class="desc-v">{{ endpoint.mode.toUpperCase() }} · {{ endpoint.base }}</span></div>
          </div>
          <p class="small muted mt8">{{ t('settings.endpointHint') }}</p>
        </a-card>
      </div>
    </div>

    <LlmSettingsModal v-model:open="llmOpen" />
  </div>
</template>
