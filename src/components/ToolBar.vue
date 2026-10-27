<script setup lang="ts">
/** 工具栏（原型图1）：高频操作一键直达；主题 / 语言在设置对话框中切换 */
import { computed } from "vue";
import { store } from "../store";
import { t } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ action: [name: string] }>();

interface ToolButton {
  name: string;
  label: string;
  icon: string;
  tip: string;
  disabled?: boolean;
  group?: number;
}

const buttons = computed<ToolButton[]>(() => [
  { name: "new", label: t("New Repository"), icon: "new-repo", tip: t("Create a new empty repository"), group: 0 },
  { name: "clone", label: t("Clone Repository"), icon: "clone", tip: t("Clone from a remote URL"), group: 0 },
  { name: "add", label: t("Add Local Repository"), icon: "folder-open", tip: t("Open an existing repository"), group: 0 },
  { name: "commit", label: t("Commit"), icon: "commit", tip: t("Commit current changes"), group: 1, disabled: !store.repoOpen },
  { name: "push", label: t("Push"), icon: "arrow-up", tip: t("Push commits to the remote"), group: 1, disabled: !store.repoOpen },
  { name: "pull", label: t("Pull"), icon: "arrow-down", tip: t("Pull and merge from the remote"), group: 1, disabled: !store.repoOpen },
  { name: "fetch", label: t("Fetch"), icon: "fetch", tip: t("Fetch remote updates only"), group: 1, disabled: !store.repoOpen },
  { name: "refresh", label: t("Refresh"), icon: "refresh", tip: t("Rescan repository status"), group: 1, disabled: !store.repoOpen },
  { name: "branches", label: t("Branches"), icon: "branch", tip: t("Manage branches"), group: 1, disabled: !store.repoOpen },
]);
</script>

<template>
  <div class="toolbar">
    <template v-for="(btn, i) in buttons" :key="btn.name">
      <span v-if="i > 0 && btn.group !== undefined && btn.group !== buttons[i - 1].group" class="tb-sep" />
      <button
        class="tb-btn"
        :disabled="btn.disabled"
        :title="btn.tip"
        @click="emit('action', btn.name)"
      >
        <IconGlyph :name="btn.icon" :size="17" />
        <span>{{ btn.label }}</span>
      </button>
    </template>
  </div>
</template>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  background: var(--bg-toolbar);
  border-bottom: 1px solid var(--divider);
  flex: none;
  flex-wrap: nowrap;
  overflow-x: auto;
}

.tb-btn {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 8px 14px;
  border: 1px solid var(--border-color);
  background: var(--bg-card);
  color: var(--text-primary);
  font-size: 13px;
  border-radius: var(--radius-md);
  cursor: pointer;
  white-space: nowrap;
  box-shadow: var(--shadow-card);
}

.tb-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}

.tb-btn:disabled {
  color: var(--text-disabled);
  cursor: default;
}

.tb-sep {
  width: 1px;
  height: 24px;
  background: var(--divider);
  flex: none;
}
</style>
