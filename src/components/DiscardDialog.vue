<script setup lang="ts">
/** 撤销修改对话框（工作区右键）：以提交对话框同款表格列出目标范围内可撤销的文件并勾选，
 * 确认后立即执行（git restore 语义，未跟踪文件删除），无二次确认弹窗 */
import { computed, onMounted, ref } from "vue";
import { store } from "../store";
import { api } from "../api";
import type { StagedFileDto } from "../api/types";
import { discardPaths } from "../tasks";
import { t, formatDateTime, formatSize } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

const target = computed(() => store.ui.discard?.target ?? "");
const isFile = computed(() => store.ui.discard?.isFile ?? false);

const preview = ref<StagedFileDto[] | null>(null);
const checked = ref<Set<string>>(new Set());

/** 目标范围：单文件取该文件；目录（空白区 = 当前浏览目录）取其下所有文件 */
const rows = computed<StagedFileDto[]>(() => {
  const all = preview.value ?? [];
  const dir = target.value;
  if (isFile.value) return all.filter((f) => f.path === dir);
  return all.filter((f) => !dir || f.path.startsWith(`${dir}/`));
});

const totalCount = computed(() => rows.value.length);
const checkedCount = computed(() => rows.value.filter((f) => checked.value.has(f.path)).length);
const allChecked = computed(() => totalCount.value > 0 && checkedCount.value === totalCount.value);
const canDiscard = computed(() => checkedCount.value > 0);

onMounted(async () => {
  try {
    // 整仓库扫描（未跟踪目录已展开为文件行），再按目标范围过滤
    preview.value = (await api.commitPreview("")).files;
    checked.value = new Set(rows.value.map((f) => f.path));
  } catch {
    emit("close");
  }
});

function toggleAll(on: boolean) {
  checked.value = on ? new Set(rows.value.map((f) => f.path)) : new Set();
}

function toggleOne(path: string, on: boolean) {
  const next = new Set(checked.value);
  if (on) next.add(path);
  else next.delete(path);
  checked.value = next;
}

async function confirmDiscard() {
  // 对话框内已有红字风险提示，点撤销立即执行（不再二次弹确认）
  const paths = rows.value.filter((f) => checked.value.has(f.path)).map((f) => f.path);
  if (await discardPaths(paths)) emit("close");
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
  <DialogBase :title="t('Discard Changes')" :width="720" @close="emit('close')">
    <div class="head-line">
      <span class="hint-line">
        {{ isFile ? t("This file's uncommitted changes will be discarded") : t("Select the changes to discard") }}
      </span>
      <span class="sel-count">{{ t("Selected") }} {{ checkedCount }} / {{ totalCount }}</span>
    </div>
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
        <div v-for="f in rows" :key="f.path" class="file-row">
          <div class="col-check">
            <input
              type="checkbox"
              :checked="checked.has(f.path)"
              @change="toggleOne(f.path, ($event.target as HTMLInputElement).checked)"
            />
          </div>
          <div class="col-path ellipsis" :title="f.path">{{ f.path }}</div>
          <div class="col-status">
            <span class="status-badge" :class="statusClass(f.kind)">
              <IconGlyph :name="statusIcon(f.kind)" :size="13" />
              <span>{{ statusLabel(f.kind) }}</span>
            </span>
          </div>
          <div class="col-size">{{ f.size == null ? "—" : formatSize(f.size) }}</div>
          <div class="col-mtime">{{ f.mtimeUnix ? formatDateTime(f.mtimeUnix) : "—" }}</div>
        </div>
        <div v-if="preview && totalCount === 0" class="empty-hint">{{ t("No changes to discard") }}</div>
      </div>
    </div>
    <div class="warn-line">{{ t("Discarded modifications will be lost permanently and cannot be recovered.") }}</div>
    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn danger" :disabled="!canDiscard" @click="confirmDiscard">
        {{ t("Discard") }}
      </button>
    </template>
  </DialogBase>
</template>

<style scoped>
.head-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
}

.hint-line {
  color: var(--text-tertiary);
  font-size: 12px;
}

.sel-count {
  font-size: 12px;
  color: var(--text-secondary);
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
  font-family: ui-monospace, Consolas, monospace;
  font-size: 12px;
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

.warn-line {
  color: var(--status-deleted);
  font-size: 12px;
  margin-top: 10px;
}
</style>
