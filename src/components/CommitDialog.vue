<script setup lang="ts">
/** 提交对话框（FR-4 扩展）：当前目录下全部可提交文件勾选列表 + 提交者 + 备注多行输入 */
import { computed, onMounted, ref } from "vue";
import { store } from "../store";
import { api } from "../api";
import type { CommitPreviewDto } from "../api/types";
import { doCommit } from "../tasks";
import { t, formatDateTime, formatSize } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

const preview = ref<CommitPreviewDto | null>(null);
const message = ref("");
const checked = ref<Set<string>>(new Set());
const committing = ref(false);

const files = computed(() => preview.value?.files ?? []);
const totalCount = computed(() => files.value.length);
const checkedCount = computed(() => files.value.filter((f) => checked.value.has(f.path)).length);
const allChecked = computed(() => totalCount.value > 0 && checkedCount.value === totalCount.value);
const canCommit = computed(
  () => !committing.value && totalCount.value > 0 && checkedCount.value > 0 && message.value.trim().length > 0,
);

/** 提交范围 = 变更页面包屑所在目录（根 = 整个仓库） */
const scopeDir = computed(() => store.currentDir);
const scopePrefix = computed(() => (scopeDir.value ? `${scopeDir.value}/` : ""));

/** 列表内展示相对当前目录的路径（悬停 title 显示完整路径） */
function displayPath(p: string): string {
  return scopePrefix.value && p.startsWith(scopePrefix.value)
    ? p.slice(scopePrefix.value.length)
    : p;
}

onMounted(async () => {
  try {
    preview.value = await api.commitPreview(scopeDir.value);
    checked.value = new Set(preview.value.files.map((f) => f.path));
  } catch {
    emit("close");
  }
});

function toggleAll(on: boolean) {
  checked.value = on ? new Set(files.value.map((f) => f.path)) : new Set();
}

function toggleOne(path: string, on: boolean) {
  const next = new Set(checked.value);
  if (on) next.add(path);
  else next.delete(path);
  checked.value = next;
}

async function submit() {
  if (!canCommit.value) return;
  committing.value = true;
  // 勾选即提交：勾选文件先暂存（未暂存内容也纳入），未勾选但已暂存的先反暂存；
  // 再按勾选路径提交（git commit -- <paths>），范围外的暂存内容不受影响
  const checkedPaths = files.value.filter((f) => checked.value.has(f.path)).map((f) => f.path);
  const unstagePaths = files.value
    .filter((f) => !checked.value.has(f.path) && f.staged)
    .map((f) => f.path);
  const prep = unstagePaths.length ? { stage: checkedPaths, unstage: unstagePaths } : undefined;
  const ok = await doCommit(message.value.trim(), false, prep, checkedPaths);
  committing.value = false;
  if (ok) emit("close");
}

function statusLabel(kind: string): string {
  const map: Record<string, string> = {
    added: "New",
    modified: "Modified",
    deleted: "Deleted",
    renamed: "Renamed",
    conflicted: "Conflicted",
  };
  return t(map[kind] ?? kind);
}

function statusTip(kind: string): string {
  const map: Record<string, string> = {
    added: "New file (will be committed)",
    modified: "Modified file (will be committed)",
    deleted: "Deleted file (will be committed)",
    renamed: "Renamed file (will be committed)",
    conflicted: "Conflicted file (will be committed as resolved)",
  };
  return t(map[kind] ?? kind);
}

function statusIcon(kind: string): string {
  if (kind === "added") return "plus";
  if (kind === "deleted") return "minus";
  if (kind === "renamed") return "edit";
  return "warn";
}

function statusClass(kind: string): string {
  return kind === "renamed" ? "modified" : kind;
}
</script>

<template>
  <DialogBase :title="t('Commit')" :width="720" :close-on-mask="false" @close="emit('close')">
    <div class="repo-line">
      <span class="repo-name">{{ preview?.name ?? store.repoName }}</span>
      <span v-if="scopeDir" class="scope-dir">{{ scopeDir }}</span>
      <span
        v-if="preview?.committer"
        class="committer"
        :title="t('To change the committer, go to Repository Settings')"
      >
        <IconGlyph name="user" :size="14" />
        <span>{{ preview.committer }}</span>
      </span>
      <span v-else-if="preview" class="committer missing">{{ t("Identity not configured") }}</span>
    </div>

    <textarea
      v-model="message"
      class="commit-msg"
      :placeholder="t('Commit message')"
      rows="4"
      :disabled="totalCount === 0"
    />

    <div class="file-list">
      <div class="list-head">
        <div class="col-check">
          <input
            type="checkbox"
            :checked="allChecked"
            :disabled="totalCount === 0"
            @change="toggleAll(($event.target as HTMLInputElement).checked)"
          />
        </div>
        <div class="col-path">{{ t("Path") }}</div>
        <div class="col-status">{{ t("Status") }}</div>
        <div class="col-size">{{ t("Size") }}</div>
        <div class="col-mtime">{{ t("Last Modified") }}</div>
      </div>
      <div class="list-body">
        <div v-for="f in files" :key="f.path" class="file-row">
          <div class="col-check">
            <input
              type="checkbox"
              :checked="checked.has(f.path)"
              @change="toggleOne(f.path, ($event.target as HTMLInputElement).checked)"
            />
          </div>
          <div class="col-path ellipsis" :title="f.path">{{ displayPath(f.path) }}</div>
          <div class="col-status">
            <span class="status-badge" :class="statusClass(f.kind)" :title="statusTip(f.kind)">
              <IconGlyph :name="statusIcon(f.kind)" :size="13" />
              <span>{{ statusLabel(f.kind) }}</span>
            </span>
          </div>
          <div class="col-size">{{ f.size == null ? "—" : formatSize(f.size) }}</div>
          <div class="col-mtime">{{ f.mtimeUnix ? formatDateTime(f.mtimeUnix) : "—" }}</div>
        </div>
        <div v-if="preview && totalCount === 0" class="empty-hint">{{ t("No changes") }}</div>
      </div>
    </div>

    <div class="dlg-actions">
      <span class="sel-count">
        <template v-if="totalCount === 0">{{ t("No changes") }}</template>
        <template v-else>{{ t("Selected") }} {{ checkedCount }} / {{ totalCount }}</template>
      </span>
      <button class="btn primary" :disabled="!canCommit" @click="submit">
        {{ t("Commit") }}
      </button>
    </div>
  </DialogBase>
</template>

<style scoped>
.repo-line {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 10px;
  min-height: 22px;
}

.repo-name {
  font-size: 14px;
  font-weight: 600;
}

.scope-dir {
  font-size: 12px;
  color: var(--text-tertiary);
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
  padding: 2px 8px;
  cursor: default;
}

.committer {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: default;
}

.committer.missing {
  color: var(--status-modified);
}

.commit-msg {
  width: 100%;
  resize: vertical;
  min-height: 80px;
  font-family: inherit;
  margin-bottom: 10px;
}

.file-list {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  overflow: hidden;
  max-height: 300px;
  display: flex;
  flex-direction: column;
}

.list-head {
  display: flex;
  align-items: center;
  padding: 7px 12px;
  border-bottom: 1px solid var(--divider);
  font-size: 12px;
  color: var(--text-tertiary);
  flex: none;
}

.list-body {
  overflow-y: auto;
}

.file-row {
  display: flex;
  align-items: center;
  padding: 6px 12px;
  border-bottom: 1px solid var(--divider);
  min-height: 34px;
}

.file-row:last-child {
  border-bottom: none;
}

.file-row:hover {
  background: var(--bg-hover);
}

.col-check {
  width: 36px;
  flex: none;
  display: flex;
  align-items: center;
}

.col-path {
  flex: 1;
  min-width: 0;
  font-size: 13px;
}

.col-status {
  width: 110px;
  flex: none;
}

.col-size {
  width: 90px;
  flex: none;
  color: var(--text-secondary);
  font-size: 12px;
}

.col-mtime {
  width: 150px;
  flex: none;
  color: var(--text-tertiary);
  font-size: 12px;
}

.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  cursor: default;
}

.status-badge.added {
  color: var(--status-untracked);
}

.status-badge.modified {
  color: var(--status-modified);
}

.status-badge.deleted {
  color: var(--status-deleted);
}

.status-badge.conflicted {
  color: var(--status-conflict);
}

.empty-hint {
  text-align: center;
  color: var(--text-tertiary);
  padding: 28px 0;
}

.dlg-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 12px;
}

.sel-count {
  font-size: 12px;
  color: var(--text-secondary);
}
</style>
