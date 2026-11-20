<script setup lang="ts">
/**
 * 生产前端外壳：登录态门禁 / 侧栏菜单 / 顶栏 / 页面切换 / Agent 右侧抽屉 / 快捷键 / 深链。
 * 业务视图全部在 views/*.vue，数据通道在 api/bridge.ts，会话与认证状态在 store.ts。
 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message, Modal } from 'ant-design-vue'
import Icon from './components/Icon.vue'
import AgentWorkbench from './components/AgentWorkbench.vue'
import LlmSettingsModal from './components/LlmSettingsModal.vue'
import LoginView from './views/LoginView.vue'
import DashboardView from './views/DashboardView.vue'
import HrView from './views/HrView.vue'
import FinanceView from './views/FinanceView.vue'
import ContractView from './views/ContractView.vue'
import AttendanceView from './views/AttendanceView.vue'
import AssetView from './views/AssetView.vue'
import AgentView from './views/AgentView.vue'
import ArchView from './views/ArchView.vue'
import { changePassword, listTable } from './api/bridge'
import { toggleLocale } from './i18n'
import {
  app, auth, closeAgentDrawer, dataVersion, doLogout, endpoint, expandFullscreen,
  openAgentDrawer, restoreAuth, runCommand, type PageKey,
} from './store'

const { t } = useI18n()

const viewMap: Record<PageKey, unknown> = {
  dashboard: DashboardView, hr: HrView, finance: FinanceView, contract: ContractView,
  attendance: AttendanceView, asset: AssetView, agent: AgentView, arch: ArchView,
}
const PAGES: PageKey[] = ['dashboard', 'hr', 'finance', 'contract', 'attendance', 'asset', 'agent', 'arch']
const currentView = computed(() => viewMap[app.activePage])

const collapsed = ref(false)
const search = ref('')
const pendingCount = ref(0)
const drawerWidth = computed(() => (app.drawerWide ? 760 : 500))
const user = computed(() => auth.user)
const avatar = computed(() => (user.value?.display_name || '?').charAt(0).toUpperCase())
const roleLabel = computed(() => (user.value ? t('role.' + user.value.role) : ''))

const menuGroups: { key: string; items: { key: PageKey; icon: string }[] }[] = [
  { key: 'overview', items: [{ key: 'dashboard', icon: 'grid' }] },
  {
    key: 'business', items: [
      { key: 'hr', icon: 'users' }, { key: 'finance', icon: 'wallet' },
      { key: 'contract', icon: 'file' }, { key: 'attendance', icon: 'clock' },
      { key: 'asset', icon: 'box' },
    ],
  },
  { key: 'intelligence', items: [{ key: 'agent', icon: 'bot' }] },
  { key: 'system', items: [{ key: 'arch', icon: 'layers' }] },
]

function go(page: PageKey): void {
  if (page === 'agent') app.agentDrawer = false
  app.activePage = page
}
function onSearch(): void {
  const text = search.value.trim()
  search.value = ''
  runCommand(text)
}
function switchLocale(): void {
  toggleLocale()
  message.success(t('toast.langSwitched'))
}
async function onLogout(): Promise<void> {
  await doLogout()
  message.success(t('auth.logout'))
}

/* ---------------- 修改密码 ---------------- */

/** 模型设置：仅管理员可见（后端还会再校验一次） */
const llmOpen = ref(false)

const pwdOpen = ref(false)
const pwdLoading = ref(false)
const pwdForm = reactive({ old: '', next: '', confirm: '' })

function openPwd(): void {
  pwdForm.old = ''
  pwdForm.next = ''
  pwdForm.confirm = ''
  pwdOpen.value = true
}

async function submitPassword(): Promise<void> {
  if (pwdForm.next !== pwdForm.confirm) {
    message.warning(t('auth.passwordMismatch'))
    return
  }
  pwdLoading.value = true
  try {
    await changePassword(pwdForm.old, pwdForm.next)
    pwdOpen.value = false
    message.success(t('auth.passwordChanged'))
    // 改密已吊销全部会话，回到登录页
    await doLogout()
  } catch (e) {
    message.error(String(e))
  } finally {
    pwdLoading.value = false
  }
}

/** 菜单待审批 badge：随写操作刷新 */
async function loadBadge(): Promise<void> {
  if (!auth.user) {
    pendingCount.value = 0
    return
  }
  try {
    const rows = await listTable('claims')
    pendingCount.value = rows.filter((r) => String((r as Record<string, unknown>).status) === 'pending').length
  } catch {
    pendingCount.value = 0
  }
}
watch(dataVersion, loadBadge)
watch(() => auth.user, loadBadge)

/** 深链：?page=finance / ?agent=drawer，便于文档与测试直达 */
function applyDeepLink(): void {
  const q = new URLSearchParams(window.location.search)
  const page = q.get('page') as PageKey | null
  if (page && PAGES.includes(page)) app.activePage = page
  if (q.get('agent') === 'drawer') openAgentDrawer()
}

onMounted(async () => {
  await restoreAuth()
  applyDeepLink()
  await loadBadge()
  void checkUpdate()
})

/**
 * 桌面端启动后静默检查更新：浏览器模式没有 Tauri IPC，直接跳过。
 * 离线 / 清单未发布 / 后端不可达都不该影响启动，因此整条链路失败只记日志。
 */
async function checkUpdate(): Promise<void> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return
  try {
    const { check } = await import('@tauri-apps/plugin-updater')
    const update = await check()
    if (!update) return
    Modal.confirm({
      title: t('update.title', { version: update.version }),
      content: t('update.desc'),
      okText: t('update.install'),
      cancelText: t('update.later'),
      onOk: async () => {
        // Windows 上 downloadAndInstall 会调起 NSIS 安装包并自动重启进程
        await update.downloadAndInstall()
      },
    })
  } catch (e) {
    console.warn('检查更新失败（不影响使用）', e)
  }
}

function onKeydown(e: KeyboardEvent): void {
  if (!auth.user) return
  if ((e.ctrlKey || e.metaKey) && String(e.key).toLowerCase() === 'k') {
    e.preventDefault()
    if (app.agentDrawer) closeAgentDrawer()
    else openAgentDrawer()
    return
  }
  if (e.key === 'Escape' && app.agentDrawer) closeAgentDrawer()
}
onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <a-config-provider :theme="{ token: { colorPrimary: '#1677ff', borderRadius: 8 } }">
    <div v-if="!auth.ready" class="oa-boot">
      <a-spin size="large" />
      <div class="muted">{{ t('auth.restoring') }}</div>
    </div>

    <LoginView v-else-if="!auth.user" />

    <div v-else class="oa-app">
      <!-- 侧边栏 -->
      <aside class="oa-sider" :class="{ 'is-collapsed': collapsed }">
        <div class="oa-logo">
          <span class="oa-logo-mark"><Icon name="flow" :size="20" /></span>
          <span>
            <div><b>{{ t('app.name') }}</b></div>
            <div class="oa-logo-sub">{{ t('app.sub') }}</div>
          </span>
        </div>
        <nav class="oa-menu">
          <template v-for="g in menuGroups" :key="g.key">
            <div class="oa-menu-group">{{ t('navGroup.' + g.key) }}</div>
            <div
              v-for="m in g.items" :key="m.key" class="oa-menu-item"
              :class="{ 'is-active': app.activePage === m.key }"
              :title="t('nav.' + m.key)" @click="go(m.key)"
            >
              <Icon :name="m.icon" :size="17" />
              <span class="m-label">{{ t('nav.' + m.key) }}</span>
              <span v-if="m.key === 'finance' && pendingCount" class="m-badge">{{ pendingCount }}</span>
            </div>
          </template>
        </nav>
        <div class="oa-sider-foot">v0.9.0 · Rig 0.46 · Tauri 2.0</div>
      </aside>

      <div class="oa-main">
        <!-- 顶栏 -->
        <header class="oa-header">
          <button
            class="oa-icon-btn"
            :title="collapsed ? t('header.expandSider') : t('header.collapse')"
            @click="collapsed = !collapsed"
          ><Icon name="grid" :size="18" /></button>
          <div>
            <div class="oa-header-title">{{ t('nav.' + app.activePage) }}</div>
            <div class="oa-header-crumb small">{{ t('app.name') }} / {{ t('nav.' + app.activePage) }}</div>
          </div>
          <span class="oa-header-spacer"></span>
          <div class="oa-search">
            <Icon name="search" :size="15" />
            <input v-model="search" :placeholder="t('header.search')" @keyup.enter="onSearch" />
          </div>
          <a-tag color="blue" :title="endpoint.base">{{ t('header.apiMode') }}: {{ endpoint.mode.toUpperCase() }}</a-tag>
          <a-button size="small" :title="t('header.lang')" @click="switchLocale">{{ t('header.langTarget') }}</a-button>
          <button class="oa-icon-btn" :title="t('header.notifications')" @click="go('finance')">
            <Icon name="bell" :size="18" /><i class="oa-dot"></i>
          </button>
          <a-button type="primary" :title="t('header.agentTip')" @click="openAgentDrawer">
            <Icon name="sparkle" :size="14" /> {{ t('header.agent') }}
          </a-button>
          <a-dropdown placement="bottomRight">
            <div class="oa-user" style="cursor: pointer">
              <span class="oa-avatar">{{ avatar }}</span>
              <span>{{ user?.display_name }}</span>
              <a-tag color="blue" style="margin-inline-end: 0">{{ roleLabel }}</a-tag>
            </div>
            <template #overlay>
              <a-menu>
                <a-menu-item key="u" disabled>{{ user?.username }} · {{ user?.employee_id }}</a-menu-item>
                <a-menu-item key="r" disabled>{{ t('auth.role') }}: {{ roleLabel }}</a-menu-item>
                <a-menu-divider />
                <a-menu-item key="pwd" @click="openPwd">{{ t('auth.changePassword') }}</a-menu-item>
                <a-menu-item v-if="user?.role === 'admin'" key="llm" @click="llmOpen = true">
                  <Icon name="settings" :size="13" /> {{ t('llm.entry') }}
                </a-menu-item>
                <a-menu-divider />
                <a-menu-item key="logout" @click="onLogout">
                  <Icon name="arrow" :size="13" /> {{ t('header.logout') }}
                </a-menu-item>
              </a-menu>
            </template>
          </a-dropdown>
        </header>

        <!-- 内容：按 activePage 切换视图。is-flush 让 Agent 页自己吃掉整块剩余视口 -->
        <main class="oa-content" :class="{ 'is-flush': app.activePage === 'agent' }">
          <component :is="currentView" :key="app.activePage" />
        </main>
      </div>

      <!-- Agent 右侧抽屉（紧凑工作台） -->
      <a-drawer
        v-model:open="app.agentDrawer" :title="t('agent.drawerTitle')"
        :width="drawerWidth" placement="right"
      >
        <div style="display: flex; gap: 8px; align-items: center; margin-bottom: 12px">
          <a-button size="small" type="primary" @click="expandFullscreen">
            <Icon name="expand" :size="13" /> {{ t('agent.fullLabel') }}
          </a-button>
          <a-button size="small" @click="app.drawerWide = !app.drawerWide">
            <Icon :name="app.drawerWide ? 'narrow' : 'widen'" :size="13" />
            {{ app.drawerWide ? t('agent.narrow') : t('agent.widen') }}
          </a-button>
          <span class="spacer"></span>
          <span class="small muted">{{ t('agent.drawerSub') }}</span>
        </div>
        <AgentWorkbench :compact="true" />
        <div class="small muted" style="margin-top: 12px">{{ t('agent.drawerHint') }}</div>
      </a-drawer>
    </div>

    <!-- 模型设置（仅管理员；Key 只在后端保存，前端只见掩码） -->
    <LlmSettingsModal v-model:open="llmOpen" />

    <!-- 修改密码（成功后吊销全部会话，需重新登录） -->
    <a-modal
      v-model:open="pwdOpen" :title="t('auth.changePassword')"
      :confirm-loading="pwdLoading" @ok="submitPassword"
    >
      <a-form layout="vertical">
        <a-form-item :label="t('auth.oldPassword')">
          <a-input-password v-model:value="pwdForm.old" autocomplete="current-password" />
        </a-form-item>
        <a-form-item :label="t('auth.newPassword')">
          <a-input-password v-model:value="pwdForm.next" autocomplete="new-password" />
        </a-form-item>
        <a-form-item :label="t('auth.newPassword')">
          <a-input-password v-model:value="pwdForm.confirm" autocomplete="new-password" />
        </a-form-item>
      </a-form>
    </a-modal>
  </a-config-provider>
</template>
