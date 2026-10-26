<script lang="ts">
// 列宽拖拽状态放模块级：FileTable 随"变更/历史"标签切换会卸载重建，模块级状态才能在切换后保留
type ResizeKey = "status" | "size" | "mtime";
const COL_DEFAULT: Record<ResizeKey, number> = { status: 140, size: 90, mtime: 170 };
const COL_MIN: Record<ResizeKey, number> = { status: 80, size: 56, mtime: 110 };
const NAME_MIN = 140; // 拖动时给名称列保底的最小宽度
const colWidths = ref<Record<ResizeKey, number>>({ ...COL_DEFAULT });
</script>

<script setup lang="ts">
/** 文件列表（图1）：名称(类型图标) / Git 状态 / 大小 / 修改时间，表头可点击排序 + 行内右键菜单（拉取/推送/提交/对比/撤销/分支操作） */
import { computed, ref, watch } from "vue";
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

// ---- 列排序：首次点击某列按降序，再点切换升序，循环往复；仅按名称排序时文件夹始终排在文件前面，其余列严格按内容混排 ----
type SortKey = "name" | "status" | "size" | "mtime";
const sortKey = ref<SortKey>("name");
const sortDir = ref<1 | -1>(1);

function setSort(key: SortKey) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 1 ? -1 : 1;
  } else {
    sortKey.value = key;
    sortDir.value = -1; // 首次点击该列 = 降序
  }
}

function byName(a: FileRowDto, b: FileRowDto): number {
  return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
}

function compareByKey(a: FileRowDto, b: FileRowDto, key: SortKey): number {
  switch (key) {
    case "name":
      return byName(a, b);
    case "status":
      return statusLabel(a.kind).localeCompare(statusLabel(b.kind)) || byName(a, b);
    case "size":
      return (a.size ?? -1) - (b.size ?? -1) || byName(a, b);
    case "mtime":
      return (a.mtimeUnix ?? 0) - (b.mtimeUnix ?? 0) || byName(a, b);
  }
}

const sortedRows = computed(() => {
  const list = [...rows.value];
  const key = sortKey.value;
  const dir = sortDir.value;
  list.sort((a, b) => {
    // 名称列：文件夹组始终在前（组内仍按当前方向排）；其他列文件夹与文件混排
    if (key === "name" && a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    return compareByKey(a, b, key) * dir;
  });
  return list;
});

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

// ---- 右键选中：右键哪行就把哪行标为选中色（与历史列表选中色一致）；左键点击列表任意处或切换目录后清除 ----
const selectedRel = ref<string | null>(null);

watch(
  () => store.currentDir,
  () => {
    selectedRel.value = null;
  },
);

function onRowContextmenu(e: MouseEvent, row: FileRowDto) {
  selectedRel.value = row.relPath;
  showContextMenu(e, rowMenu(row, (r) => emit("openDiff", r)));
}

// 空白处右键：清除所有选中高亮（行右键被 showContextMenu 的 stopPropagation 拦截，不会到这里；
// 事件继续冒泡到 WorkArea 弹空白区菜单）
function onBlankContextmenu() {
  selectedRel.value = null;
}

// ---- 列宽拖拽：名称列自适应吸收剩余宽度；状态/大小/修改时间三列固定宽，拖表头分界线调整，双击分界线复位 ----
const tableEl = ref<HTMLElement | null>(null);

// 列宽经 CSS 变量下发，表头与数据行共用同一套列类名，天然对齐
const colVars = computed(() => ({
  "--col-status": `${colWidths.value.status}px`,
  "--col-size": `${colWidths.value.size}px`,
  "--col-mtime": `${colWidths.value.mtime}px`,
}));

function startResize(key: ResizeKey, e: MouseEvent) {
  e.preventDefault();
  const startX = e.clientX;
  const startW = colWidths.value[key];
  const others = (Object.keys(COL_DEFAULT) as ResizeKey[])
    .filter((k) => k !== key)
    .reduce((s, k) => s + colWidths.value[k], 0);
  // 28 = 表头/行左右 padding 合计；名称列保底后即为该列上限
  const maxW = tableEl.value
    ? Math.max(COL_MIN[key], tableEl.value.clientWidth - 28 - NAME_MIN - others)
    : startW;
  const onMove = (ev: MouseEvent) => {
    colWidths.value[key] = Math.min(maxW, Math.max(COL_MIN[key], startW + ev.clientX - startX));
  };
  const onUp = () => {
    document.body.classList.remove("col-resizing");
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
  };
  document.body.classList.add("col-resizing");
  window.addEventListener("mousemove", onMove);
  window.addEventListener("mouseup", onUp);
}

function resetWidths() {
  colWidths.value = { ...COL_DEFAULT };
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
  <div ref="tableEl" class="file-table" :style="colVars" @click="selectedRel = null" @contextmenu="onBlankContextmenu">
    <div class="table-head">
      <div class="col-name th-sort" :class="{ on: sortKey === 'name' }" :title="t('Click to sort')" @click="setSort('name')">
        <span>{{ t("Name") }}</span>
        <IconGlyph v-if="sortKey === 'name'" name="chevronDown" :size="11" class="sort-arrow" :class="{ asc: sortDir === 1 }" />
        <span class="col-rz" @mousedown="startResize('status', $event)" @click.stop @dblclick.stop="resetWidths"></span>
      </div>
      <div class="col-status th-sort" :class="{ on: sortKey === 'status' }" :title="t('Click to sort')" @click="setSort('status')">
        <span>{{ t("Status") }}</span>
        <IconGlyph v-if="sortKey === 'status'" name="chevronDown" :size="11" class="sort-arrow" :class="{ asc: sortDir === 1 }" />
        <span class="col-rz" @mousedown="startResize('size', $event)" @click.stop @dblclick.stop="resetWidths"></span>
      </div>
      <div class="col-size th-sort" :class="{ on: sortKey === 'size' }" :title="t('Click to sort')" @click="setSort('size')">
        <span>{{ t("Size") }}</span>
        <IconGlyph v-if="sortKey === 'size'" name="chevronDown" :size="11" class="sort-arrow" :class="{ asc: sortDir === 1 }" />
        <span class="col-rz" @mousedown="startResize('mtime', $event)" @click.stop @dblclick.stop="resetWidths"></span>
      </div>
      <div class="col-mtime th-sort" :class="{ on: sortKey === 'mtime' }" :title="t('Click to sort')" @click="setSort('mtime')">
        <span>{{ t("Last Modified") }}</span>
        <IconGlyph v-if="sortKey === 'mtime'" name="chevronDown" :size="11" class="sort-arrow" :class="{ asc: sortDir === 1 }" />
      </div>
    </div>

    <div class="table-body">
      <div
        v-for="row in sortedRows"
        :key="row.relPath"
        class="table-row"
        :class="{ dir: row.isDir, ignored: row.kind === 'ignored', selected: row.relPath === selectedRel }"
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
  align-items: stretch;
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
  align-items: stretch;
  padding: 7px 14px;
  border-bottom: 1px solid var(--divider);
  cursor: default;
}

.table-row:hover {
  background: var(--bg-hover);
}

/* 右键选中的行：与历史列表选中色一致；置于 hover 之后，悬停时选中色不被覆盖 */
.table-row.selected {
  background: var(--bg-card-selected);
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
  padding-right: 12px;
}

.file-icon {
  color: var(--text-tertiary);
  flex: none;
}

.file-icon.dirIcon {
  color: var(--status-modified);
}

/* 列分隔线比行分隔线（--divider）深一档；单元格内边距让内容与分隔线保持间距 */
.col-status {
  width: var(--col-status, 140px);
  flex: none;
  display: flex;
  align-items: center;
  padding: 0 12px;
  border-left: 1px solid var(--border-color);
}

.col-size {
  width: var(--col-size, 90px);
  flex: none;
  display: flex;
  align-items: center;
  padding: 0 12px;
  border-left: 1px solid var(--border-color);
  color: var(--text-secondary);
  font-size: 12px;
}

.col-mtime {
  width: var(--col-mtime, 170px);
  flex: none;
  display: flex;
  align-items: center;
  padding-left: 12px;
  border-left: 1px solid var(--border-color);
  color: var(--text-tertiary);
  font-size: 12px;
}

/* 表头可点击排序：悬停变色、激活列主题色 + 方向箭头（升序箭头翻转向上） */
.th-sort {
  position: relative;
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  user-select: none;
}

.th-sort:hover {
  color: var(--text-secondary);
}

.th-sort.on {
  color: var(--accent);
}

.sort-arrow {
  flex: none;
  transition: transform 0.15s;
}

.sort-arrow.asc {
  transform: rotate(180deg);
}

/* 列宽拖拽把手：骑在分界线上，透明、悬停显色；负向 top/bottom 补满表头上下 padding 的抓取区 */
.col-rz {
  position: absolute;
  top: -8px;
  bottom: -8px;
  right: -6px;
  width: 11px;
  cursor: col-resize;
  z-index: 2;
}

.col-rz::after {
  content: "";
  position: absolute;
  top: 0;
  bottom: 0;
  left: 5px;
  width: 1px;
  background: transparent;
}

.col-rz:hover::after {
  background: var(--accent);
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

<style>
/* 拖动列宽期间：全局锁定光标并禁止选中文本（scoped 作用不到 body，单独放开） */
body.col-resizing,
body.col-resizing * {
  cursor: col-resize !important;
  user-select: none !important;
}
</style>
