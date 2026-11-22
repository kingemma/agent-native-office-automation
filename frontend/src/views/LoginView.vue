<script setup lang="ts">
/**
 * 登录页：账号密码 → 后端签发会话令牌。
 * 演示账号直接点选填充；角色权限由服务端返回的 permissions 决定后续 UI 裁剪。
 */
import { reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from '../components/Icon.vue'
import { auth, doLogin, endpoint, testEndpoints } from '../store'
import { toggleLocale } from '../i18n'
import { TOOL_CATALOG } from '../model'

const { t } = useI18n()

const form = reactive({ username: 'evan', password: 'oa123456' })
const serverOpen = ref(false)
const dlOpen = ref(false)
const testMsg = ref('')
const testOk = ref(false)

/** 各平台安装包下载：URL 缺省指向 GitHub Releases，可用 VITE_RELEASE_BASE 覆盖 */
const RELEASE = import.meta.env.VITE_RELEASE_BASE
  ?? 'https://github.com/kingemma/agent-native-office-automation/releases/latest'
const downloads: { id: string; icon: string; href: string }[] = [
  { id: 'windows', icon: 'monitor', href: RELEASE },
  { id: 'macos', icon: 'laptop', href: '' },
  { id: 'linux', icon: 'cpu', href: '' },
  { id: 'android', icon: 'phone', href: '' },
  { id: 'ios', icon: 'phone', href: '' },
  { id: 'server', icon: 'disc', href: `${RELEASE}/download/oa-server.exe` },
]

/** 手动测试两套地址：结果只做提示，登录时仍会自动重新探测 */
async function testServer(): Promise<void> {
  const result = await testEndpoints()
  testOk.value = result.lan || result.wan
  testMsg.value = result.lan
    ? t('auth.lanOk')
    : result.wan
      ? t('auth.wanOnly')
      : t('auth.noneOk')
}
const demos = [
  { username: 'evan', key: 'demoAdmin', tone: 'blue' },
  { username: 'wuhao', key: 'demoFinance', tone: 'green' },
  { username: 'fengxue', key: 'demoHr', tone: 'purple' },
  { username: 'lina', key: 'demoEmployee', tone: 'orange' },
]

function fill(username: string): void {
  form.username = username
  form.password = 'oa123456'
}

async function submit(): Promise<void> {
  await doLogin(form.username, form.password)
}
</script>

<template>
  <div class="login-page">
    <div class="login-brand">
      <div class="lb-logo">
        <span class="lb-mark"><Icon name="flow" :size="26" /></span>
        <div>
          <h1>{{ t('app.name') }}</h1>
          <p>{{ t('app.slogan') }}</p>
        </div>
      </div>
      <div class="lb-hero">
        <h2><Icon name="sparkle" :size="22" /> {{ t('agent.heroTitle') }}</h2>
        <p>{{ t('agent.heroDesc') }}</p>
      </div>
      <ul class="lb-points">
        <li><Icon name="shield" :size="15" /> {{ t('auth.tip') }}</li>
        <li><Icon name="flow" :size="15" /> {{ t('agent.chip2', { n: TOOL_CATALOG.length }) }}</li>
        <li><Icon name="activity" :size="15" /> {{ t('agent.chip3') }}</li>
      </ul>
      <button class="lb-lang" @click="toggleLocale">{{ t('header.langTarget') }}</button>
    </div>

    <div class="login-panel">
      <div class="login-card">
        <div class="lc-head">
          <h2>{{ t('auth.title') }}</h2>
          <p class="muted">{{ t('auth.sub') }}</p>
        </div>

        <!-- 服务端地址：局域网优先，探测失败自动切外网；配置保存在本机 -->
        <div class="lc-server">
          <button type="button" class="lc-server-head" :title="endpoint.base" @click="serverOpen = !serverOpen">
            <Icon name="globe" :size="14" />
            <span>{{ t('auth.server') }}</span>
            <span class="lc-mode" :class="'mode-' + endpoint.mode">
              {{ endpoint.mode === 'wan' ? t('auth.modeWan') : t('auth.modeLan') }}
            </span>
            <span class="lc-chevron">{{ serverOpen ? '▴' : '▾' }}</span>
          </button>
          <div v-if="serverOpen" class="lc-server-body">
            <a-form layout="vertical">
              <a-form-item :label="t('auth.lanAddr')">
                <a-input v-model:value="endpoint.lan" placeholder="http://192.168.1.10:8787" />
              </a-form-item>
              <a-form-item :label="t('auth.wanAddr')">
                <a-input v-model:value="endpoint.wan" placeholder="https://oa.example.com" />
              </a-form-item>
            </a-form>
            <div class="lc-server-foot">
              <a-button size="small" :loading="endpoint.checking" @click="testServer">{{ t('auth.test') }}</a-button>
              <span v-if="testMsg" class="lc-test" :class="testOk ? 'ok' : 'bad'">{{ testMsg }}</span>
            </div>
            <p class="lc-hint">{{ t('auth.autoHint') }}</p>
            <p class="lc-base">{{ endpoint.base }}</p>
          </div>
        </div>

        <a-alert
          v-if="auth.error" type="error" show-icon class="mb16"
          :message="auth.error"
        />

        <a-form layout="vertical" @submit.prevent="submit">
          <a-form-item :label="t('auth.username')">
            <a-input
              v-model:value="form.username" size="large" autocomplete="username"
              :placeholder="t('auth.username')"
            >
              <template #prefix><Icon name="user" :size="14" /></template>
            </a-input>
          </a-form-item>
          <a-form-item :label="t('auth.password')">
            <a-input-password
              v-model:value="form.password" size="large" autocomplete="current-password"
              :placeholder="t('auth.password')"
            >
              <template #prefix><Icon name="shield" :size="14" /></template>
            </a-input-password>
          </a-form-item>
          <a-button
            type="primary" size="large" block html-type="submit"
            :loading="auth.loading" class="lc-submit"
          >
            {{ auth.loading ? t('auth.loggingIn') : t('auth.submit') }}
          </a-button>
        </a-form>

        <div class="lc-demo">
          <div class="small muted">{{ t('auth.demoTitle') }}</div>
          <div class="lc-demo-chips">
            <button
              v-for="d in demos" :key="d.username" class="demo-chip"
              :class="'tone-' + d.tone" @click="fill(d.username)"
            >
              {{ t('auth.' + d.key) }}
            </button>
          </div>
        </div>

        <!-- 各平台客户端下载 -->
        <div class="lc-dl">
          <button type="button" class="lc-server-head" @click="dlOpen = !dlOpen">
            <Icon name="disc" :size="14" />
            <span>{{ t('auth.downloadTitle') }}</span>
            <span class="lc-chevron">{{ dlOpen ? '▴' : '▾' }}</span>
          </button>
          <div v-if="dlOpen" class="lc-dl-body">
            <p class="lc-hint">{{ t('auth.downloadHint') }}</p>
            <div class="lc-dl-list">
              <template v-for="d in downloads" :key="d.id">
                <a v-if="d.href" :href="d.href" target="_blank" rel="noopener" class="lc-dl-item">
                  <Icon :name="d.icon" :size="15" />
                  <span>{{ t('downloads.' + d.id) }}</span>
                </a>
                <span v-else class="lc-dl-item disabled">
                  <Icon :name="d.icon" :size="15" />
                  <span>{{ t('downloads.' + d.id) }}</span>
                  <em class="lc-dl-soon">{{ t('downloads.comingSoon') }}</em>
                </span>
              </template>
            </div>
          </div>
        </div>
      </div>
      <div class="login-foot">v0.9.0 · Rig 0.46 · Tauri 2.0 · Axum</div>
    </div>
  </div>
</template>

<style scoped>
.login-page {
  height: 100vh;
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(420px, 1fr);
  background: var(--oa-bg);
}
.login-brand {
  position: relative;
  padding: 56px 64px;
  display: flex;
  flex-direction: column;
  gap: 34px;
  color: #fff;
  background: linear-gradient(140deg, #0b3aa8 0%, #1677ff 46%, #722ed1 100%);
  overflow: hidden;
}
.login-brand::after {
  content: '';
  position: absolute;
  width: 560px;
  height: 560px;
  right: -190px;
  bottom: -230px;
  border-radius: 50%;
  background: radial-gradient(circle, rgba(255, 255, 255, 0.24), transparent 62%);
}
.lb-logo { display: flex; align-items: center; gap: 14px; }
.lb-mark {
  width: 52px;
  height: 52px;
  border-radius: 14px;
  display: grid;
  place-items: center;
  background: rgba(255, 255, 255, 0.16);
  border: 1px solid rgba(255, 255, 255, 0.28);
}
.lb-logo h1 { margin: 0; font-size: 24px; letter-spacing: 1px; }
.lb-logo p { margin: 2px 0 0; font-size: 11px; letter-spacing: 2.4px; opacity: 0.78; }
.lb-hero { max-width: 520px; position: relative; z-index: 1; }
.lb-hero h2 { display: flex; align-items: center; gap: 8px; font-size: 27px; margin: 0 0 12px; }
.lb-hero p { margin: 0; line-height: 1.75; font-size: 13.5px; opacity: 0.86; }
.lb-points { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 12px; position: relative; z-index: 1; }
.lb-points li { display: flex; align-items: center; gap: 9px; font-size: 12.5px; opacity: 0.9; }
.lb-lang {
  position: absolute;
  top: 22px;
  right: 24px;
  z-index: 2;
  background: rgba(255, 255, 255, 0.16);
  border: 1px solid rgba(255, 255, 255, 0.32);
  color: #fff;
  border-radius: 8px;
  padding: 5px 13px;
  cursor: pointer;
  font-size: 12.5px;
}
.lb-lang:hover { background: rgba(255, 255, 255, 0.26); }
.login-panel { display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 40px 32px; }
.login-card {
  width: 100%;
  max-width: 408px;
  background: var(--oa-card);
  border: 1px solid var(--oa-border);
  border-radius: 16px;
  padding: 32px 30px 26px;
  box-shadow: 0 22px 60px rgba(20, 40, 90, 0.1);
}
.lc-head h2 { margin: 0 0 4px; font-size: 21px; }
.lc-head p { margin: 0 0 22px; font-size: 12.5px; letter-spacing: 0.4px; }
.lc-submit { height: 44px; font-weight: 600; }
.lc-demo { margin-top: 22px; padding-top: 18px; border-top: 1px dashed var(--oa-border); }
.lc-demo-chips { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }
.demo-chip {
  border: 1px solid transparent;
  border-radius: 999px;
  padding: 4px 12px;
  font-size: 12px;
  cursor: pointer;
  background: #f5f6f8;
  color: #333;
}
.demo-chip:hover { border-color: currentColor; }
.tone-blue { background: #e6f4ff; color: #0958d9; }
.tone-green { background: #f6ffed; color: #389e0d; }
.tone-purple { background: #f9f0ff; color: #722ed1; }
.tone-orange { background: #fff7e6; color: #d46b08; }
.login-foot { margin-top: 18px; font-size: 11px; color: var(--oa-muted); letter-spacing: 0.4px; }
.lc-server { margin-bottom: 16px; border: 1px solid var(--oa-border); border-radius: 10px; overflow: hidden; }
.lc-server-head {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: #f7f8fa;
  border: 0;
  cursor: pointer;
  font-size: 12.5px;
  color: var(--oa-text, #1f2329);
  text-align: left;
}
.lc-mode { border-radius: 999px; padding: 1px 8px; font-size: 11px; }
.lc-mode.mode-lan { background: #e6f4ff; color: #0958d9; }
.lc-mode.mode-wan { background: #f6ffed; color: #389e0d; }
.lc-chevron { margin-left: auto; color: var(--oa-muted); }
.lc-server-body { padding: 12px; border-top: 1px dashed var(--oa-border); }
.lc-server-body :deep(.ant-form-item) { margin-bottom: 10px; }
.lc-server-foot { display: flex; align-items: center; gap: 10px; margin-bottom: 6px; }
.lc-test { font-size: 12px; }
.lc-test.ok { color: #389e0d; }
.lc-test.bad { color: #cf1322; }
.lc-hint { margin: 0 0 4px; font-size: 11.5px; color: var(--oa-muted); line-height: 1.6; }
.lc-base { margin: 0; font-size: 11px; color: var(--oa-muted); font-family: monospace; word-break: break-all; }
.lc-dl { margin-top: 16px; border: 1px solid var(--oa-border); border-radius: 10px; overflow: hidden; }
.lc-dl-body { padding: 12px; border-top: 1px dashed var(--oa-border); }
.lc-dl-list { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px; }
.lc-dl-item {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 5px 10px; font-size: 12.5px; border-radius: 8px;
  border: 1px solid var(--oa-border); background: #f7f8fa; color: var(--oa-text, #1f2329);
  text-decoration: none; transition: all 0.15s;
}
.lc-dl-item:not(.disabled):hover { border-color: var(--oa-primary); color: var(--oa-primary); }
.lc-dl-item.disabled { opacity: 0.55; cursor: default; }
.lc-dl-soon { font-style: normal; font-size: 10.5px; color: var(--oa-muted); }
@media (max-width: 980px) {
  .login-page { grid-template-columns: 1fr; }
  .login-brand { display: none; }
}
</style>
