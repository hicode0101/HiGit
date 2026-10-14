<script setup lang="ts">
/** 主内容区（图1）：变更 / 历史 双标签 + 面包屑 + 文件列表（提交走工具栏对话框） */
import { store } from "../store";
import { t } from "../i18n";
import Breadcrumb from "./Breadcrumb.vue";
import FileTable from "./FileTable.vue";
import HistoryView from "./HistoryView.vue";
import WelcomeView from "./WelcomeView.vue";

const emit = defineEmits<{
  openDiff: [row: { relPath: string; kind: string }];
  openCommitDiff: [oid: string, path: string];
}>();
</script>

<template>
  <div class="work-area">
    <template v-if="store.repoOpen">
      <div class="tabs">
        <button class="tab" :class="{ active: store.tab === 'changes' }" @click="store.tab = 'changes'">
          {{ t("Changes") }}
        </button>
        <button class="tab" :class="{ active: store.tab === 'history' }" @click="store.tab = 'history'">
          {{ t("History") }}
        </button>
      </div>
      <template v-if="store.tab === 'changes'">
        <Breadcrumb />
        <FileTable class="file-area" @open-diff="(row) => emit('openDiff', row)" />
      </template>
      <HistoryView
        v-else
        class="history"
        @open-commit-diff="(oid, path) => emit('openCommitDiff', oid, path)"
      />
    </template>
    <WelcomeView v-else />
  </div>
</template>

<style scoped>
.work-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg-window);
}

.tabs {
  display: flex;
  gap: 4px;
  padding: 10px 18px 0;
  flex: none;
}

.tab {
  border: 1px solid var(--border-color);
  border-bottom: none;
  background: var(--bg-card);
  color: var(--text-secondary);
  padding: 7px 22px;
  font-size: 13px;
  border-radius: var(--radius-md) var(--radius-md) 0 0;
  cursor: pointer;
}

.tab.active {
  color: var(--accent);
  font-weight: 600;
  background: var(--bg-card);
  box-shadow: inset 0 -2px 0 var(--accent);
}

.file-area {
  flex: 1;
  min-height: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  margin: 0 18px 14px;
  background: var(--bg-card);
}

.history {
  flex: 1;
  min-height: 0;
}
</style>
