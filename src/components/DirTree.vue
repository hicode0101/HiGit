<script setup lang="ts">
/** 目录树（图1下半）：懒加载展开、.git 特殊标识、点击目录定位文件列表 */
import { store, navigate, refreshTree } from "../store";
import type { TreeRowDto } from "../api/types";
import IconGlyph from "./IconGlyph.vue";

function rowIndent(row: TreeRowDto): string {
  return `${8 + row.level * 18}px`;
}

async function onToggle(row: TreeRowDto) {
  if (row.gitMeta || !row.hasChildren) return; // .git 恒折叠；叶子目录没有下一级
  const p = row.absPath;
  if (store.treeExpanded.has(p)) {
    store.treeExpanded.delete(p);
  } else {
    store.treeExpanded.add(p);
  }
  await refreshTree();
}

function onRowClick(row: TreeRowDto) {
  // 点击目录行：定位文件列表到该目录（FR-2.1）
  const rel = toRel(row.absPath);
  if (row.gitMeta) return;
  void navigate(rel);
}

/** 双击目录：展开下一级目录列表（已展开则不动） */
async function onRowDblclick(row: TreeRowDto) {
  if (row.gitMeta || !row.hasChildren) return;
  if (store.treeExpanded.has(row.absPath)) return;
  store.treeExpanded.add(row.absPath);
  await refreshTree();
}

/** 展开箭头只给可展开的行（.git 恒折叠、叶子目录不显示） */
function showChevron(row: TreeRowDto): boolean {
  return !row.gitMeta && row.hasChildren;
}

/** 图标区分：实心文件夹 = 还有子目录，描边 = 叶子目录，.git 保持特殊图标 */
function folderIcon(row: TreeRowDto): string {
  if (row.gitMeta) return "folder-git";
  return row.hasChildren ? "folder-solid" : "folder";
}

function toRel(abs: string): string {
  // absPath / repoPath 是平台原生分隔符（Windows 反斜杠），统一转成 / 再转相对，
  // 与文件列表 relPath、面包屑分段（split("/"))、状态条目前缀匹配保持一致
  const a = abs.replace(/\\/g, "/");
  const root = store.repoPath.replace(/\\/g, "/").replace(/\/+$/, "");
  return a === root ? "" : a.slice(root.length + 1);
}
</script>

<template>
  <div class="dir-tree">
    <div
      v-for="row in store.treeRows"
      :key="row.absPath"
      class="tree-row"
      :style="{ paddingLeft: rowIndent(row) }"
      :class="{ gitMeta: row.gitMeta }"
      @click="onRowClick(row)"
      @dblclick="onRowDblclick(row)"
    >
      <span class="tw-spacer" :class="{ clickable: showChevron(row) }" @click.stop="onToggle(row)">
        <IconGlyph
          v-if="showChevron(row)"
          :name="store.treeExpanded.has(row.absPath) ? 'chevronDown' : 'chevronRight'"
          :size="12"
          class="tw-chevron"
        />
      </span>
      <IconGlyph :name="folderIcon(row)" :size="16" class="tw-icon" />
      <span class="ellipsis tw-name">{{ row.name }}</span>
    </div>
  </div>
</template>

<style scoped>
.dir-tree {
  overflow: auto;
  padding: 2px 6px 10px;
  flex: 1;
  font-size: 13px;
}

.tree-row {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  padding-right: 8px;
}

.tree-row:hover {
  background: var(--bg-hover);
}

.tree-row.gitMeta {
  color: var(--text-tertiary);
}

.tw-spacer {
  width: 16px;
  height: 100%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  color: var(--text-tertiary);
  cursor: default;
}

.tw-spacer.clickable {
  cursor: pointer;
}

.tw-chevron {
  flex: none;
}

.tw-icon {
  color: var(--accent);
  flex: none;
}

.tree-row.gitMeta .tw-icon {
  color: var(--text-tertiary);
}

.tw-name {
  margin-left: 4px;
}
</style>
