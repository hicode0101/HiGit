<script setup lang="ts">
/** 目录树（图1下半）：懒加载展开、.git 特殊标识、点击目录定位文件列表 */
import { store, navigate, refreshTree } from "../store";
import type { TreeRowDto } from "../api/types";
import IconGlyph from "./IconGlyph.vue";

function rowIndent(row: TreeRowDto): string {
  return `${8 + row.level * 18}px`;
}

async function onToggle(row: TreeRowDto) {
  if (row.gitMeta) return; // .git 元目录恒折叠
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

function toRel(abs: string): string {
  // 前端把绝对路径转成相对路径（分隔符已由后端统一为 /）
  const root = store.repoPath.replace(/\/+$/, "");
  return abs === root ? "" : abs.slice(root.length + 1);
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
    >
      <span class="tw-spacer" @click.stop="onToggle(row)">
        <IconGlyph
          v-if="!row.gitMeta"
          :name="store.treeExpanded.has(row.absPath) ? 'chevronDown' : 'chevronRight'"
          :size="12"
          class="tw-chevron"
        />
      </span>
      <IconGlyph :name="row.gitMeta ? 'folder-git' : 'folder'" :size="16" class="tw-icon" />
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
