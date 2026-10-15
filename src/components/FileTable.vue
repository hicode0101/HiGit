<script setup lang="ts">
/** 文件列表（图1）：名称(类型图标) / Git 状态 / 大小 / 修改时间 + 行内右键菜单（拉取/推送/提交/对比/撤销/分支操作） */
import { computed } from "vue";
import { store, navigate } from "../store";
import { t, formatDateTime, formatSize } from "../i18n";
import { showContextMenu } from "../ctxmenu";
import { rowMenu } from "../workmenu";
import type { FileRowDto } from "../api/types";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ openDiff: [row: FileRowDto] }>();

const showIgnored = computed(() => store.config?.showIgnored ?? false);

const rows = computed(() =>
  store.fileRows.filter((r) => showIgnored.value || r.kind !== "ignored"),
);

function statusLabel(kind: string): string {
  const map: Record<string, string> = {
    clean: "Unmodified",
    modified: "Modified",
    staged: "Staged",
    untracked: "New",
    deleted: "Deleted",
    renamed: "Modified",
    conflicted: "Conflicted",
    ignored: "Ignored",
  };
  return t(map[kind] ?? kind);
}

function statusClass(kind: string): string {
  return kind === "renamed" ? "modified" : kind;
}

async function onRowActivate(row: FileRowDto) {
  if (row.isDir) {
    await navigate(row.relPath);
  } else {
    emit("openDiff", row);
  }
}

function onRowContextmenu(e: MouseEvent, row: FileRowDto) {
  showContextMenu(e, rowMenu(row, (r) => emit("openDiff", r)));
}

function fileIcon(row: FileRowDto): string {
  if (row.isDir) return "folder";
  const ext = row.name.includes(".") ? row.name.split(".").pop()!.toLowerCase() : "";
  if (["png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp"].includes(ext)) return "file-image";
  if (["json", "toml", "yaml", "yml", "xml", "ini", "conf", "config", "properties"].includes(ext))
    return "file-config";
  if (["md", "txt", "doc", "docx", "pdf", "rst"].includes(ext)) return "file-doc";
  return "file-code";
}
</script>

<template>
  <div class="file-table">
    <div class="table-head">
      <div class="col-name">{{ t("Name") }}</div>
      <div class="col-status">{{ t("Status") }}</div>
      <div class="col-size">{{ t("Size") }}</div>
      <div class="col-mtime">{{ t("Last Modified") }}</div>
    </div>

    <div class="table-body">
      <div
        v-for="row in rows"
        :key="row.relPath"
        class="table-row"
        :class="{ dir: row.isDir, ignored: row.kind === 'ignored' }"
        @dblclick="onRowActivate(row)"
        @contextmenu.prevent="onRowContextmenu($event, row)"
      >
        <div class="col-name">
          <IconGlyph :name="fileIcon(row)" :size="17" class="file-icon" :class="{ dirIcon: row.isDir }" />
          <span class="ellipsis">{{ row.name }}</span>
        </div>
        <div class="col-status">
          <span class="status-badge" :class="statusClass(row.kind)">
            <IconGlyph :name="row.kind === 'clean' || row.kind === 'staged' ? 'check' : row.kind === 'untracked' ? 'plus' : row.kind === 'deleted' ? 'minus' : row.kind === 'conflicted' ? 'warn' : 'warn'" :size="13" />
            <span>{{ statusLabel(row.kind) }}</span>
          </span>
        </div>
        <div class="col-size">
          {{ row.isDir ? "" : row.size == null ? "—" : formatSize(row.size) }}
        </div>
        <div class="col-mtime">
          {{ row.mtimeUnix ? formatDateTime(row.mtimeUnix) : "—" }}
        </div>
      </div>
      <div v-if="rows.length === 0" class="empty-hint">{{ t("No changes") }}</div>
    </div>
  </div>
</template>

<style scoped>
.file-table {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  flex: 1;
}

.table-head {
  display: flex;
  align-items: center;
  padding: 8px 14px;
  border-bottom: 1px solid var(--divider);
  font-size: 12px;
  color: var(--text-tertiary);
  flex: none;
}

.table-body {
  overflow-y: auto;
  flex: 1;
}

.table-row {
  display: flex;
  align-items: center;
  padding: 7px 14px;
  border-bottom: 1px solid var(--divider);
  cursor: default;
}

.table-row:hover {
  background: var(--bg-hover);
}

.table-row.dir {
  cursor: pointer;
}

.table-row.ignored {
  color: var(--text-tertiary);
}

.col-name {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 13px;
}

.file-icon {
  color: var(--text-tertiary);
  flex: none;
}

.file-icon.dirIcon {
  color: var(--status-modified);
}

.col-status {
  width: 140px;
  flex: none;
}

.col-size {
  width: 90px;
  flex: none;
  color: var(--text-secondary);
  font-size: 12px;
}

.col-mtime {
  width: 170px;
  flex: none;
  color: var(--text-tertiary);
  font-size: 12px;
}

.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}

.status-badge.clean {
  color: var(--status-clean);
}

.status-badge.modified {
  color: var(--status-modified);
}

.status-badge.staged {
  color: var(--status-staged);
}

.status-badge.untracked {
  color: var(--status-untracked);
}

.status-badge.deleted {
  color: var(--status-deleted);
}

.status-badge.conflicted {
  color: var(--status-conflict);
}

.status-badge.ignored {
  color: var(--status-ignored);
}

.empty-hint {
  text-align: center;
  color: var(--text-tertiary);
  padding: 40px 0;
}
</style>
