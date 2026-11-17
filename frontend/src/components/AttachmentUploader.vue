<script setup lang="ts">
/**
 * 附件 / 发票图片上传。
 * 只依赖 bridge 的统一上传通道（base64 JSON）：桌面端与浏览器端行为一致，
 * 前端拿不到也存不了本地路径，服务端统一落盘并做类型/大小/权限校验。
 */
import { onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { message } from 'ant-design-vue'
import Icon from './Icon.vue'
import {
  deleteAttachment, listAttachments, readAttachment, uploadAttachment,
  type Attachment,
} from '../api/bridge'

const props = defineProps<{
  entityType: string
  entityId: string
  readonly?: boolean
}>()

const { t } = useI18n()
const rows = ref<Attachment[]>([])
const busy = ref(false)
const error = ref('')
const inputEl = ref<HTMLInputElement | null>(null)
const previewOpen = ref(false)
const previewUrl = ref('')
const previewName = ref('')
const previewIsImage = ref(false)

async function load(): Promise<void> {
  if (!props.entityId) {
    rows.value = []
    return
  }
  try {
    rows.value = await listAttachments(props.entityType, props.entityId)
    error.value = ''
  } catch (e) {
    error.value = String(e)
  }
}

onMounted(load)
watch(() => [props.entityType, props.entityId], load)

function pick(): void {
  inputEl.value?.click()
}

async function onFiles(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement
  const picked = Array.from(input.files || [])
  input.value = ''
  if (!picked.length || !props.entityId) return
  busy.value = true
  error.value = ''
  try {
    for (const file of picked) await uploadAttachment(file, props.entityType, props.entityId)
    message.success(t('attach.uploaded'))
    await load()
  } catch (e) {
    error.value = String(e)
    message.error(String(e))
  } finally {
    busy.value = false
  }
}

async function preview(row: Attachment): Promise<void> {
  try {
    const content = await readAttachment(row.id)
    previewUrl.value = content.data_url
    previewName.value = content.file_name
    previewIsImage.value = content.mime.startsWith('image/')
    previewOpen.value = true
  } catch (e) {
    message.error(String(e))
  }
}

async function remove(row: Attachment): Promise<void> {
  try {
    await deleteAttachment(row.id)
    await load()
  } catch (e) {
    message.error(String(e))
  }
}

function sizeText(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}
</script>

<template>
  <div class="attach-box">
    <div class="attach-head">
      <h4>{{ t('attach.title') }}</h4>
      <span class="small muted">{{ t('attach.hint') }}</span>
      <span class="spacer"></span>
      <a-button v-if="!readonly" size="small" :loading="busy" @click="pick">
        <Icon name="plus" :size="13" /> {{ t('attach.add') }}
      </a-button>
      <input
        ref="inputEl"
        class="attach-input"
        type="file"
        multiple
        accept="image/png,image/jpeg,image/webp,image/gif,application/pdf"
        @change="onFiles"
      />
    </div>

    <div v-if="!rows.length" class="small muted">{{ t('attach.empty') }}</div>
    <div v-for="row in rows" :key="row.id" class="attach-row">
      <span class="attach-name" @click="preview(row)">{{ row.file_name }}</span>
      <span class="small muted">{{ sizeText(row.size_bytes) }} · {{ row.uploaded_by }}</span>
      <span class="spacer"></span>
      <a-button type="link" size="small" @click="preview(row)">{{ t('attach.preview') }}</a-button>
      <a-button v-if="!readonly" type="link" size="small" danger @click="remove(row)">
        {{ t('attach.remove') }}
      </a-button>
    </div>
    <div v-if="error" class="attach-error">{{ error }}</div>

    <a-modal v-model:open="previewOpen" :title="previewName" :footer="null" :width="720">
      <img v-if="previewIsImage" :src="previewUrl" :alt="previewName" class="attach-image" />
      <iframe v-else :src="previewUrl" :title="previewName" class="attach-frame"></iframe>
      <div class="attach-download">
        <a :href="previewUrl" :download="previewName">{{ t('attach.download') }}</a>
      </div>
    </a-modal>
  </div>
</template>

<style scoped>
.attach-box {
  margin-top: 18px;
  padding-top: 14px;
  border-top: 1px solid var(--oa-border, #e5e7eb);
}
.attach-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  flex-wrap: wrap;
}
.attach-head h4 {
  margin: 0;
  font-size: 13.5px;
}
.attach-input {
  display: none;
}
.attach-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 0;
  border-bottom: 1px dashed var(--oa-border, #eef0f3);
  min-width: 0;
}
.attach-row:last-of-type {
  border-bottom: 0;
}
.attach-name {
  font-size: 13px;
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
}
.attach-name:hover {
  color: var(--oa-primary, #1677ff);
}
.attach-error {
  margin-top: 6px;
  font-size: 12px;
  color: #cf1322;
}
.attach-image {
  max-width: 100%;
}
.attach-frame {
  width: 100%;
  height: 60vh;
  border: 0;
}
.attach-download {
  margin-top: 10px;
  font-size: 12.5px;
}
</style>
