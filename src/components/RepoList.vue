<script setup lang="ts">
/** 仓库列表（图1）：名称 / 路径 / 分支 / 待提交变更徽标；支持拖拽调整顺序 */
import { ref } from "vue";
import { store, openRepo, refreshRepos, showError } from "../store";
import type { RepoSummaryDto } from "../api/types";
import { api } from "../api";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ contextMenu: [path: string, e: MouseEvent] }>();

const dragPath = ref<string | null>(null);

function displayName(r: RepoSummaryDto): string {
  return r.alias || r.name;
}

function onCardClick(r: RepoSummaryDto) {
  if (r.path !== store.repoPath) void openRepo(r.path);
}

function onDragStart(e: DragEvent, r: RepoSummaryDto) {
  dragPath.value = r.path;
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", r.path);
  }
}

/** 拖拽经过卡片：按鼠标处于上/下半区决定插入位置，实时重排 */
function onDragOver(e: DragEvent, r: RepoSummaryDto) {
  const drag = dragPath.value;
  if (!drag) return;
  e.preventDefault(); // 只要拖拽在进行就放行（含悬停自身），避免出现禁止光标
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
  if (drag === r.path) return;
  const target = e.currentTarget as HTMLElement | null;
  if (!target) return;
  const rect = target.getBoundingClientRect();
  const before = e.clientY < rect.top + rect.height / 2;
  const list = store.repos;
  const from = list.findIndex((x) => x.path === drag);
  const overIdx = list.findIndex((x) => x.path === r.path);
  if (from < 0 || overIdx < 0) return;
  let to = before ? overIdx : overIdx + 1;
  if (from < to) to -= 1; // 先移除再插入的索引修正
  if (to === from) return;
  const [item] = list.splice(from, 1);
  list.splice(to, 0, item);
}

/** 拖到列表空白区：移到末尾 */
function onListDragOver(e: DragEvent) {
  const drag = dragPath.value;
  if (!drag || e.target !== e.currentTarget) return;
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
  const list = store.repos;
  const from = list.findIndex((x) => x.path === drag);
  if (from < 0 || from === list.length - 1) return;
  const [item] = list.splice(from, 1);
  list.push(item);
}

/** 拖拽结束（松手 / Esc 取消 / 拖出窗口）：按当前视觉顺序持久化 */
function onDragEnd() {
  const drag = dragPath.value;
  dragPath.value = null;
  if (!drag) return;
  api.reorderRepos(store.repos.map((r) => r.path)).catch((e) => {
    showError(e as Error);
    void refreshRepos(); // 恢复后端真实顺序
  });
}
</script>

<template>
  <div class="repo-list" @dragover="onListDragOver" @drop.prevent>
    <div
      v-for="r in store.repos"
      :key="r.path"
      class="repo-card"
      :class="{ selected: r.path === store.repoPath, dragging: r.path === dragPath }"
      draggable="true"
      @click="onCardClick(r)"
      @contextmenu="(e: MouseEvent) => emit('contextMenu', r.path, e)"
      @dragstart="onDragStart($event, r)"
      @dragover="onDragOver($event, r)"
      @dragend="onDragEnd"
    >
      <span class="repo-folder">
        <IconGlyph name="folder" :size="20" />
      </span>
      <div class="repo-info">
        <div class="repo-name-row">
          <span class="repo-name ellipsis">{{ displayName(r) }}</span>
          <span v-if="r.branch" class="repo-branch">{{ r.branch }}</span>
        </div>
        <div class="repo-path ellipsis">{{ r.path }}</div>
      </div>
      <span v-if="r.dirty > 0" class="repo-badge" :title="`${r.dirty} changes`">{{ r.dirty }}</span>
    </div>
    <div v-if="store.repos.length === 0" class="repo-empty">—</div>
  </div>
</template>

<style scoped>
.repo-list {
  overflow-y: auto;
  padding: 2px 8px 10px;
  flex: 1;
}

.repo-card {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border-radius: var(--radius-md);
  cursor: pointer;
  position: relative;
}

.repo-card:hover {
  background: var(--bg-hover);
}

.repo-card.dragging {
  opacity: 0.4;
}

.repo-card.selected {
  background: var(--accent);
  color: #fff;
}

.repo-folder {
  color: var(--accent);
  flex: none;
  display: inline-flex;
}

.repo-card.selected .repo-folder {
  color: #fff;
}

.repo-info {
  flex: 1;
  min-width: 0;
}

.repo-name-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.repo-name {
  font-weight: 600;
  font-size: 13px;
}

.repo-branch {
  font-size: 11px;
  color: var(--text-tertiary);
}

.repo-path {
  font-size: 11px;
  color: var(--text-tertiary);
  margin-top: 2px;
}

.repo-card.selected .repo-branch,
.repo-card.selected .repo-path {
  color: rgba(255, 255, 255, 0.85);
}

.repo-badge {
  flex: none;
  min-width: 20px;
  height: 20px;
  padding: 0 6px;
  border-radius: 10px;
  background: var(--status-modified);
  color: #fff;
  font-size: 11px;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.repo-card.selected .repo-badge {
  background: rgba(255, 255, 255, 0.25);
}

.repo-empty {
  text-align: center;
  color: var(--text-tertiary);
  padding: 14px;
}
</style>
