<script setup lang="ts">
/** HiGit 主布局：标题栏 / 菜单栏 / 工具栏 / 侧栏 / 主区 / 状态栏 + 全局对话框与快捷键 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  store,
  bootstrap,
  watchConfig,
  toast,
} from "./store";
import {
  initTasks,
  cancelCurrentJob,
  addLocalRepoAction,
  openSyncDialog,
  openCommitDialog,
} from "./tasks";
import { t } from "./i18n";
import TitleBar from "./components/TitleBar.vue";
import MenuBar from "./components/MenuBar.vue";
import ToolBar from "./components/ToolBar.vue";
import SideBar from "./components/SideBar.vue";
import WorkArea from "./components/WorkArea.vue";
import StatusBar from "./components/StatusBar.vue";
import ContextMenu from "./components/ContextMenu.vue";
import Toasts from "./components/Toasts.vue";
import NewRepoDialog from "./components/NewRepoDialog.vue";
import CloneDialog from "./components/CloneDialog.vue";
import SyncDialog from "./components/SyncDialog.vue";
import CommitDialog from "./components/CommitDialog.vue";
import DiscardDialog from "./components/DiscardDialog.vue";
import BranchPickerDialog from "./components/BranchPickerDialog.vue";
import BranchesDialog from "./components/BranchesDialog.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import RepoSettingsDialog from "./components/RepoSettingsDialog.vue";
import CredentialsDialog from "./components/CredentialsDialog.vue";
import ErrorDialog from "./components/ErrorDialog.vue";
import ConfirmInputDialog from "./components/ConfirmInputDialog.vue";
import DiffView from "./components/DiffView.vue";
import MiscDialogs from "./components/MiscDialogs.vue";

function onToolbarAction(name: string) {
  switch (name) {
    case "new":
      store.ui.newRepo = true;
      break;
    case "clone":
      store.ui.clone = true;
      break;
    case "add":
      void addLocalRepoAction();
      break;
    case "commit":
      void openCommitDialog();
      break;
    case "push":
      void openSyncDialog("push");
      break;
    case "pull":
      void openSyncDialog("pull");
      break;
    case "fetch":
      void openSyncDialog("fetch");
      break;
    case "refresh":
      window.dispatchEvent(new CustomEvent("higit:refresh"));
      break;
    case "branches":
      store.ui.branches = true;
      break;
  }
}

async function doRefresh() {
  window.dispatchEvent(new CustomEvent("higit:refresh"));
}

async function onSyncClick(kind: "push" | "pull") {
  // 状态栏同步按钮同样走同步对话框
  await openSyncDialog(kind);
}

// ---------- 全局快捷键（FR §5.6） ----------

function onKeydown(e: KeyboardEvent) {
  if (e.key === "F5") {
    e.preventDefault();
    void doRefresh();
    return;
  }
  if (e.ctrlKey || e.metaKey) {
    const k = e.key.toLowerCase();
    if (k === "n" && !e.shiftKey) {
      e.preventDefault();
      store.ui.newRepo = true;
    } else if (k === "c" && e.shiftKey) {
      e.preventDefault();
      store.ui.clone = true;
    } else if (k === "b") {
      e.preventDefault();
      store.sidebarVisible = !store.sidebarVisible;
    } else if (k === ",") {
      e.preventDefault();
      store.ui.settings = true;
    } else if (k === "enter") {
      e.preventDefault();
      void openCommitDialog();
    }
    return;
  }
  if (e.key === "Escape") {
    // 依序关闭最上层对话框 / 差异视图
    if (store.ui.error) store.ui.error = null;
    else if (store.ui.credentials) store.ui.credentials = null;
    else if (store.ui.diff) store.ui.diff = null;
    else if (store.ui.commitDiff) store.ui.commitDiff = null;
    else if (store.ui.progress) void cancelCurrentJob();
    else if (store.ui.syncDialog) window.dispatchEvent(new CustomEvent("higit:sync-esc"));
    else if (store.ui.commitDialog) store.ui.commitDialog = false;
    else if (store.ui.discard) store.ui.discard = null;
    else if (store.ui.branchOp) store.ui.branchOp = null;
    else if (store.ui.clone) store.ui.clone = false;
    else if (store.ui.newRepo) store.ui.newRepo = false;
    else if (store.ui.branches) store.ui.branches = false;
    else if (store.ui.settings) store.ui.settings = false;
    else if (store.ui.repoSettingsId) store.ui.repoSettingsId = null;
    else if (store.ui.legend) store.ui.legend = false;
    else if (store.ui.about) store.ui.about = false;
    else if (store.ui.shortcuts) store.ui.shortcuts = false;
    return;
  }
}

const sidebarVisible = computed(() => store.sidebarVisible);

onMounted(async () => {
  watchConfig();
  await bootstrap();
  initTasks();
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("contextmenu", suppressNativeContextMenu);
  window.addEventListener("higit:refresh", () => {
    void (async () => {
      const { refreshStatus, refreshTree, refreshRepos } = await import("./store");
      await Promise.all([refreshStatus(), refreshTree(), refreshRepos()]);
    })();
  });
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("contextmenu", suppressNativeContextMenu);
});

function openDiff(row: { relPath: string; kind: string }) {
  store.ui.diff = { path: row.relPath, spec: row.kind === "staged" ? "staged" : "workdir" };
}

function onCommitDiff(oid: string, path: string) {
  store.ui.commitDiff = { oid, path };
}

// 禁用 WebView2 默认右键菜单（返回/刷新/打印…）；自带右键菜单（仓库列表等）内部已 preventDefault，不受影响
function suppressNativeContextMenu(e: MouseEvent) {
  e.preventDefault();
}
</script>

<template>
  <div class="app-root">
    <TitleBar />
    <MenuBar />
    <ToolBar @action="onToolbarAction" />
    <div class="main">
      <SideBar v-if="sidebarVisible" />
      <WorkArea @open-diff="openDiff" @open-commit-diff="onCommitDiff" />
    </div>
    <StatusBar
      @open-branches="store.ui.branches = true"
      @sync-click="onSyncClick"
      @open-legend="store.ui.legend = true"
    />

    <!-- 对话框层 -->
    <NewRepoDialog v-if="store.ui.newRepo" @close="store.ui.newRepo = false" />
    <CloneDialog v-if="store.ui.clone" @close="store.ui.clone = false" />
    <SyncDialog v-if="store.ui.syncDialog" @close="store.ui.syncDialog = null" />
    <CommitDialog v-if="store.ui.commitDialog" @close="store.ui.commitDialog = false" />
    <DiscardDialog v-if="store.ui.discard" @close="store.ui.discard = null" />
    <BranchPickerDialog v-if="store.ui.branchOp" @close="store.ui.branchOp = null" />
    <BranchesDialog v-if="store.ui.branches" @close="store.ui.branches = false" />
    <SettingsDialog v-if="store.ui.settings" @close="store.ui.settings = false" />
    <RepoSettingsDialog
      v-if="store.ui.repoSettingsId"
      @close="store.ui.repoSettingsId = null"
    />
    <CredentialsDialog v-if="store.ui.credentials" @close="store.ui.credentials = null" />
    <ErrorDialog v-if="store.ui.error" @close="store.ui.error = null" />
    <ConfirmInputDialog />
    <DiffView
      v-if="store.ui.diff"
      :work="store.ui.diff"
      :commit="null"
      @close="store.ui.diff = null"
    />
    <DiffView
      v-if="store.ui.commitDiff"
      :work="null"
      :commit="store.ui.commitDiff"
      @close="store.ui.commitDiff = null"
    />
    <MiscDialogs />
    <ContextMenu />
    <Toasts />
  </div>
</template>

<style scoped>
.app-root {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.main {
  flex: 1;
  display: flex;
  overflow: hidden;
  min-height: 0;
}
</style>
