<script setup lang="ts">
/** 左侧边栏（图1/图2）：上半仓库列表（右键菜单），下半目录树；两区可独立折叠，收起后另一区占满全高 */
import { computed, ref } from "vue";
import { store, refreshRepos, showError, showConfirm } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import { showContextMenu, type CtxMenuItem } from "../ctxmenu";
import IconGlyph from "./IconGlyph.vue";
import RepoList from "./RepoList.vue";
import DirTree from "./DirTree.vue";

const reposCollapsed = ref(false);
const treeCollapsed = ref(false);

const hasRepo = computed(() => store.repoOpen);

async function syncRepo(kind: "push" | "pull" | "fetch", path: string) {
  const { openSyncDialog } = await import("../tasks");
  // 与工具栏一致：打开同步对话框（先切换到所点的仓库）
  await openSyncDialog(kind, path);
}

function repoMenu(path: string): CtxMenuItem[] {
  return [
    { label: t("Pull"), icon: "arrow-down", action: () => void syncRepo("pull", path) },
    { label: t("Push"), icon: "arrow-up", action: () => void syncRepo("push", path) },
    { label: t("Fetch"), icon: "fetch", action: () => void syncRepo("fetch", path) },
    { separator: true },
    {
      label: t("Open Containing Folder"),
      icon: "folder",
      action: () => api.openInExplorer(path).catch(showError),
    },
    {
      label: t("Remove from List"),
      icon: "close",
      danger: true,
      action: () => {
        const repo = store.repos.find((r) => r.path === path);
        const name = repo ? repo.alias || repo.name : path;
        return showConfirm({
          title: t("Remove from List"),
          message: t('Remove "{name}" from the list? Files on disk will not be deleted.', { name }),
          confirmLabel: t("Remove from List"),
          danger: true,
        }).then(async (ok) => {
          if (!ok) return;
          try {
            await api.removeRepo(path);
            if (store.repoPath === path) await closeRepoAfterRemove();
            await refreshRepos();
          } catch (e) {
            showError(e as Error);
          }
        });
      },
    },
    { separator: true },
    {
      label: t("Settings"),
      icon: "settings",
      action: () => {
        // 按唯一 id 打开设置（同名仓库也各自独立）
        store.ui.repoSettingsId = store.repos.find((r) => r.path === path)?.id ?? null;
      },
    },
  ];
}

async function closeRepoAfterRemove() {
  store.repoOpen = false;
  store.repoPath = "";
  store.status = null;
  store.fileRows = [];
  store.treeRows = [];
  store.currentDir = "";
  try {
    await api.closeRepo();
  } catch {
    /* ignore */
  }
}

function toggleSection(which: "repos" | "tree") {
  if (which === "repos") reposCollapsed.value = !reposCollapsed.value;
  else treeCollapsed.value = !treeCollapsed.value;
  setTimeout(() => window.dispatchEvent(new Event("resize")), 0);
}
</script>

<template>
  <div class="sidebar">
    <div class="side-section repos" :class="{ collapsed: reposCollapsed }">
      <div class="side-head" @click="toggleSection('repos')">
        <span>{{ t("Repositories") }}</span>
        <IconGlyph name="chevronDown" :size="14" :class="{ collapsed: reposCollapsed }" />
      </div>
      <RepoList v-show="!reposCollapsed" @context-menu="(p: string, e: MouseEvent) => showContextMenu(e, repoMenu(p))" />
    </div>
    <div class="side-section tree" :class="{ collapsed: treeCollapsed }">
      <div class="side-head" @click="toggleSection('tree')">
        <span>{{ t("Directory") }}</span>
        <IconGlyph name="chevronDown" :size="14" :class="{ collapsed: treeCollapsed }" />
      </div>
      <template v-if="!treeCollapsed">
        <DirTree v-if="hasRepo" />
        <div v-else class="side-empty">{{ t("No repository open") }}</div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.sidebar {
  width: 280px;
  min-width: 200px;
  display: flex;
  flex-direction: column;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--divider);
  overflow: hidden;
  flex: none;
}

.side-section {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* 展开的一方占据剩余全部高度；折叠的一方收缩到仅剩标题行 */
.side-section.repos {
  flex: 1.1;
  min-height: 0;
  border-bottom: 1px solid var(--divider);
}

.side-section.repos.collapsed {
  flex: none;
}

.side-section.tree {
  flex: 1;
  min-height: 0;
}

.side-section.tree.collapsed {
  flex: none;
}

.side-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  cursor: pointer;
  user-select: none;
  flex: none;
}

.side-head svg {
  color: var(--text-tertiary);
  transition: transform 0.15s;
}

.side-head svg.collapsed {
  transform: rotate(-90deg);
}

.side-empty {
  padding: 18px 14px;
  color: var(--text-tertiary);
  font-size: 12px;
}
</style>
