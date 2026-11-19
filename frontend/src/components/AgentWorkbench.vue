<script setup lang="ts">
/**
 * Agent 工作台：抽屉（compact=true）与全屏（compact=false）复用同一组件，
 * 且共享 store.ts 里的同一份响应式会话状态。
 */
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import Icon from './Icon.vue'
import { currentLocale } from '../i18n'
import { QUICK_GROUPS, TOOL_CATALOG } from '../model'
import { renderMarkdown } from '../markdown'
import { json, L } from '../utils'
import {
  clearSession, dock, loadTools, previewPlan, sendAgent, session,
} from '../store'

withDefaults(defineProps<{ compact?: boolean }>(), { compact: false })

const { t } = useI18n()
const draft = ref('')
const traceOpen = ref(false)
const chatBody = ref<HTMLElement | null>(null)
const caps = ['capHr', 'capFin', 'capCon', 'capAtt', 'capAst']

/* ---------------- 输入框 ---------------- */

const inputEl = ref<HTMLTextAreaElement | null>(null)

/** 随内容增高；超过 200px 才出现内部滚动 */
function resizeInput(): void {
  const el = inputEl.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${Math.min(el.scrollHeight, 200)}px`
}

/* ---------------- 语音输入（Web Speech API） ---------------- */

interface SpeechResult {
  isFinal: boolean
  0: { transcript: string }
}
interface SpeechEvent {
  resultIndex: number
  results: { length: number; [index: number]: SpeechResult }
}
interface Recognizer {
  lang: string
  interimResults: boolean
  continuous: boolean
  start: () => void
  stop: () => void
  abort: () => void
  onresult: ((event: SpeechEvent) => void) | null
  onerror: ((event: { error?: string }) => void) | null
  onend: (() => void) | null
}
type RecognizerCtor = new () => Recognizer

/** 只有 Chrome / Edge / WebView2 提供实现；不支持的浏览器直接不渲染麦克风 */
function recognizerCtor(): RecognizerCtor | null {
  const scope = window as unknown as {
    SpeechRecognition?: RecognizerCtor
    webkitSpeechRecognition?: RecognizerCtor
  }
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null
}

const voiceSupported = ref(false)
const listening = ref(false)
const voiceError = ref('')
let recognizer: Recognizer | null = null

function stopVoice(): void {
  recognizer?.stop()
  recognizer = null
  listening.value = false
}

function toggleVoice(): void {
  if (listening.value) {
    stopVoice()
    return
  }
  const Ctor = recognizerCtor()
  if (!Ctor) return

  voiceError.value = ''
  // 识别结果追加到已有草稿后面，不覆盖用户手打的内容
  const prefix = draft.value && !/\s$/.test(draft.value) ? `${draft.value} ` : draft.value
  let finals = ''

  const rec = new Ctor()
  rec.lang = currentLocale() === 'en-US' ? 'en-US' : 'zh-CN'
  rec.interimResults = true
  // 单句模式：说完一句自动收尾，避免长时间占用麦克风
  rec.continuous = false
  rec.onresult = (event) => {
    let interim = ''
    for (let i = event.resultIndex; i < event.results.length; i++) {
      const result = event.results[i]
      const text = result?.[0]?.transcript ?? ''
      if (result.isFinal) finals += text
      else interim += text
    }
    draft.value = `${prefix}${finals}${interim}`
    void nextTick(resizeInput)
  }
  rec.onerror = (event) => {
    voiceError.value = event.error === 'not-allowed' ? t('agent.voiceDenied') : t('agent.voiceFailed')
  }
  rec.onend = () => {
    listening.value = false
    recognizer = null
    void nextTick(resizeInput)
  }

  recognizer = rec
  listening.value = true
  try {
    rec.start()
  } catch {
    stopVoice()
    voiceError.value = t('agent.voiceFailed')
  }
}

const planSteps = computed(() =>
  session.plan && Array.isArray(session.plan.steps) ? session.plan.steps : [],
)
const toolCount = computed(() => session.tools.length || TOOL_CATALOG.length)

/** 思考块的折叠状态：默认展开，用户手动收起的才记下来 */
const thinkClosed = reactive<Record<number, boolean>>({})
function toggleThink(id: number): void {
  thinkClosed[id] = !thinkClosed[id]
}
/** 有占位消息在流式接收时，不再额外显示单独的"思考中"气泡 */
const hasStreaming = computed(() => session.messages.some((m) => m.streaming))

function onQuick(item: { cmd: { zh: string; en: string } }): void {
  void sendAgent(L(item.cmd))
  draft.value = ''
}
function onSend(): void {
  const text = draft.value.trim()
  if (!text) return
  draft.value = ''
  void sendAgent(text)
}
function onPreview(): void {
  void previewPlan(draft.value)
}
function clearError(): void {
  session.error = ''
}

function resultText(v: unknown): string {
  if (v == null) return ''
  if (typeof v === 'string') return v
  if (typeof v === 'object') {
    const o = v as Record<string, unknown>
    return String(o.msg ?? o.message ?? o.reply ?? json(v))
  }
  return String(v)
}


function scrollChat(): void {
  nextTick(() => {
    if (chatBody.value) chatBody.value.scrollTop = chatBody.value.scrollHeight
  })
}
watch([() => session.messages.length, () => session.running], scrollChat)
// 思考步骤是在同一条消息里持续追加的，长度变化也要跟着滚到底
watch(() => session.messages.map((m) => m.thinking?.length || 0).join(','), scrollChat)
// 正文字节流式增长时同理
watch(() => session.messages.reduce((n, m) => n + m.text.length, 0), scrollChat)
watch(draft, () => void nextTick(resizeInput))

onMounted(() => {
  if (!session.tools.length) void loadTools()
  if (session.trace.length) traceOpen.value = true
  voiceSupported.value = recognizerCtor() !== null
  scrollChat()
  void nextTick(resizeInput)
})

onUnmounted(() => {
  try {
    recognizer?.abort()
  } catch {
    // 未启动时 abort 会抛 InvalidStateError，忽略即可
  }
  recognizer = null
})
</script>

<template>
  <div class="agent-workbench" :class="{ 'is-compact': compact }">
    <!-- 全屏形态不再铺 Hero 横幅：整页高度留给对话区，说明性内容收进能力清单 -->
    <a-alert
      v-if="session.error" class="agent-error" type="warning" show-icon closable
      :message="t('error.backendDown')" :description="session.error" @close="clearError"
    />

    <div class="agent-layout" :class="{ 'is-compact': compact }">
      <!-- 对话 -->
      <a-card class="agent-chat" :title="t('agent.chatTitle')">
        <template #extra>
          <a-button v-if="!compact" type="text" size="small" @click="dock">
            <Icon name="panel" :size="13" /> {{ t('agent.dock') }}
          </a-button>
          <a-button type="text" size="small" @click="clearSession">
            <Icon name="refresh" :size="13" /> {{ t('agent.clear') }}
          </a-button>
        </template>
        <div class="agent-chat-body" ref="chatBody">
          <div v-if="!session.messages.length" class="agent-msg">
            <div class="agent-msg-av bot"><Icon name="bot" :size="16" /></div>
            <div class="agent-bubble">{{ t('agent.welcome') }}</div>
          </div>

          <div
            v-for="m in session.messages" :key="m.id" class="agent-msg"
            :class="{ 'is-user': m.role === 'user' }"
          >
            <div class="agent-msg-av" :class="m.role === 'user' ? 'usr' : 'bot'">
              <Icon :name="m.role === 'user' ? 'user' : 'bot'" :size="16" />
            </div>
            <div class="agent-bubble">
              <!-- 思考过程：流式时逐条追加并强制展开，结束后可折叠回看 -->
              <div
                v-if="m.role === 'agent' && m.thinking && m.thinking.length"
                class="think-box" :class="{ 'is-live': m.streaming }"
              >
                <button type="button" class="think-head" @click="toggleThink(m.id)">
                  <a-spin v-if="m.streaming" size="small" />
                  <Icon v-else name="flow" :size="13" />
                  <span>{{ m.streaming ? t('agent.thinkingLive') : t('agent.thinkingTitle') }}</span>
                  <a-tag v-if="!m.streaming">{{ m.thinking.length }} {{ t('agent.thinkSteps') }}</a-tag>
                  <span class="spacer"></span>
                  <Icon :name="m.streaming || !thinkClosed[m.id] ? 'up' : 'down'" :size="13" />
                </button>
                <div v-if="m.streaming || !thinkClosed[m.id]" class="think-body">
                  <div v-for="(s, i) in m.thinking" :key="i" class="think-row">
                    <span class="think-stage">{{ t('agent.stage.' + s.stage) }}</span>
                    <span class="think-text">{{ s.text }}</span>
                  </div>
                </div>
              </div>

              <div v-if="m.trace && m.trace.length" class="bubble-title">
                <Icon name="checked" :size="13" /> {{ t('agent.planDone') }} · {{ m.trace.length }} {{ t('agent.stepsDone') }}
              </div>
              <!-- 用户输入保持纯文本；Agent 回复按 markdown 渲染（已过 DOMPurify） -->
              <div v-if="m.role === 'user'">{{ m.text }}</div>
              <div v-else class="md-body" v-html="renderMarkdown(m.text)"></div>
              <div v-if="m.mode" class="small muted" style="margin-top: 6px">
                {{ t('agent.modeLabel') }}: {{ m.mode }}
              </div>
            </div>
          </div>

          <div v-if="session.running && !hasStreaming" class="agent-msg">
            <div class="agent-msg-av bot"><Icon name="bot" :size="16" /></div>
            <div class="agent-bubble"><a-spin size="small" /> {{ t('agent.thinking') }}</div>
          </div>
        </div>

        <!-- 快捷指令（6 个业务域）：全屏形态只在空会话时占用高度，有对话后把空间让给回复 -->
        <div v-if="compact || !session.messages.length" class="agent-quick">
          <div class="agent-quick-title">{{ t('agent.quickTitle') }}</div>
          <div class="agent-quick-chips">
            <template v-for="g in QUICK_GROUPS" :key="g.key">
              <button
                v-for="it in g.items" :key="it.label.zh" class="agent-chip"
                :disabled="session.running" @click="onQuick(it)"
              >
                <Icon :name="g.icon" :size="11" /> {{ L(g.label) }} · {{ L(it.label) }}
              </button>
            </template>
          </div>
        </div>

        <!-- 输入区：文字框独占一行占满宽度，工具条在下方 -->
        <div class="agent-composer">
          <textarea
            ref="inputEl" v-model="draft" :placeholder="t('agent.placeholder')"
            :class="{ 'is-listening': listening }"
            @keydown.enter.exact.prevent="onSend"
          ></textarea>
          <div class="agent-composer-bar">
            <button
              v-if="voiceSupported" type="button" class="composer-mic"
              :class="{ 'is-on': listening }"
              :title="listening ? t('agent.voiceStop') : t('agent.voice')"
              @click="toggleVoice"
            >
              <Icon name="mic" :size="16" />
            </button>
            <span v-if="listening" class="composer-hint">{{ t('agent.voiceListening') }}</span>
            <span v-else-if="voiceError" class="composer-hint is-err">{{ voiceError }}</span>
            <span class="spacer"></span>
            <a-button :loading="session.planLoading" @click="onPreview">{{ t('agent.previewPlan') }}</a-button>
            <a-button type="primary" :disabled="session.running" @click="onSend">
              <Icon name="send" :size="14" /> {{ session.running ? t('agent.sending') : t('agent.send') }}
            </a-button>
          </div>
        </div>
      </a-card>

      <!-- 右栏：折叠开关 + 执行轨迹 + 能力清单。
           全屏时整列独立滚动，高度不参与对话区分配，回复区因此能撑满视口 -->
      <div class="agent-side">
        <button v-if="compact" class="drawer-trace-toggle" @click="traceOpen = !traceOpen">
          <Icon name="flow" :size="15" />
          <span>{{ traceOpen ? t('agent.hideTrace') : t('agent.showTrace') }}</span>
          <a-tag v-if="session.trace.length" color="blue">{{ session.trace.length }} {{ t('agent.steps') }}</a-tag>
          <span class="spacer"></span>
          <Icon :name="traceOpen ? 'up' : 'down'" :size="14" />
        </button>

        <!-- 执行轨迹 / 计划预览 -->
        <a-card v-if="!compact || traceOpen" class="trace-panel" :title="t('agent.traceTitle')">
          <template #extra>
            <a-tag :color="session.running ? 'blue' : 'green'">
              {{ session.running ? t('agent.sending') : toolCount + ' ' + t('agent.toolCount') }}
            </a-tag>
          </template>
          <div class="trace-body">
            <div v-if="session.plan" class="plan-box">
              <div class="plan-head">{{ t('agent.planTitle') }} · {{ planSteps.length }} {{ t('agent.steps') }}</div>
              <div class="plan-step" v-for="(s, i) in planSteps" :key="i">
                <span class="idx">{{ i + 1 }}</span>
                <code>{{ s.tool }}</code>
                <span class="muted small">{{ json(s.args) }}</span>
              </div>
              <div v-if="session.plan.reply" class="small muted" style="margin-top: 8px">{{ session.plan.reply }}</div>
            </div>

            <div v-if="!session.trace.length && !session.plan" class="trace-empty">
              <Icon name="flow" :size="28" />
              <span>{{ session.planLoading ? t('agent.thinking') : t('agent.traceEmpty') }}</span>
            </div>

            <div class="trace-tool" v-for="(tr, i) in session.trace" :key="i">
              <div class="trace-tool-head">
                <span class="idx">{{ i + 1 }}</span>
                <span class="nm">{{ tr.tool }}</span>
                <span class="spacer"></span>
                <a-tag :color="tr.ok ? 'green' : 'red'">{{ tr.ok ? 'OK' : 'ERR' }}</a-tag>
              </div>
              <div class="trace-tool-body">
                <div class="trace-kv">{{ json(tr.args) }}</div>
                <div class="trace-res" :class="{ 'is-err': !tr.ok }">
                  <Icon :name="tr.ok ? 'checked' : 'alert'" :size="14" />
                  <span>{{ resultText(tr.result) }}</span>
                </div>
              </div>
            </div>
          </div>
        </a-card>

        <!-- 全屏形态：能力清单（原页面底部的说明性内容，挪进右栏避免挤压对话区） -->
        <a-card v-if="!compact" class="mt16 cap-card" :title="t('agent.capability')">
          <div class="cap-grid">
            <div class="cap-item" v-for="c in caps" :key="c">
              <a-alert type="info" :message="t('agent.' + c)" />
            </div>
            <div class="cap-item">
              <a-alert type="warning" :message="t('agent.confirmNeeded')" />
            </div>
          </div>
        </a-card>
      </div>
    </div>
  </div>
</template>
