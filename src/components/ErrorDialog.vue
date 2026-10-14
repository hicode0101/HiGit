<script setup lang="ts">
/** 错误对话框（FR-11.2）：本地化文案 + 可复制原始详情 */
import { computed } from "vue";
import { store } from "../store";
import { t } from "../i18n";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import DialogBase from "./DialogBase.vue";

const emit = defineEmits<{ close: [] }>();

const multi = computed(() => store.ui.error?.message.includes("\n") ?? false);

async function copyRaw() {
  try {
    await writeText(store.ui.error?.raw ?? "");
  } catch {
    /* ignore */
  }
}
</script>

<template>
  <DialogBase :title="t('Error')" :width="480" @close="emit('close')">
    <div class="err-msg" :class="{ pre: multi }">{{ store.ui.error?.message }}</div>
    <div v-if="store.ui.error?.raw" class="raw">
      <div class="raw-head">
        <span>Details</span>
        <button class="btn mini" @click="copyRaw">{{ t("Copy") }}</button>
      </div>
      <pre class="raw-body">{{ store.ui.error?.raw }}</pre>
    </div>

    <template #footer>
      <button class="btn primary" @click="emit('close')">{{ t("OK") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.err-msg {
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
}

.err-msg.pre {
  white-space: pre-wrap;
  font-family: var(--font-mono);
  font-size: 12px;
}

.raw {
  margin-top: 12px;
}

.raw-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--text-tertiary);
  margin-bottom: 4px;
}

.btn.mini {
  padding: 2px 10px;
  font-size: 11px;
}

.raw-body {
  font-family: var(--font-mono);
  font-size: 11px;
  background: var(--bg-code);
  border-radius: var(--radius-sm);
  padding: 10px;
  max-height: 160px;
  overflow: auto;
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}
</style>
