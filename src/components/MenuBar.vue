<script setup lang="ts">
/** 菜单栏（FR §5.3）：文件 / 视图 / 工具 / 帮助；Windows/Linux 支持 Alt+助记键（macOS 无此惯例） */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { store, persistConfig, showError, toast } from "../store";
import { addLocalRepoAction } from "../tasks";
import { t, currentLocale } from "../i18n";

interface MenuItem {
  label?: string;
  shortcut?: string;
  separator?: boolean;
  checked?: boolean;
  disabled?: boolean;
  action?: () => void;
  children?: () => MenuItem[];
}

interface MenuDef {
  name: string;
  label: string;
  /** Alt 助记键（Windows / Linux） */
  accel: string;
  items: MenuItem[];
}

const openMenu = ref<string | null>(null);

/** macOS 菜单无 Alt 助记键惯例（用 Cmd 组合键），Windows/Linux 显示并响应 Alt+字母 */
const isMac = /mac|iphone|ipad/i.test(navigator.platform);
const isZh = computed(() => currentLocale() === "zh-CN");

async function exportDiagnostics() {
  try {
    const path = await import("../api").then((m) => m.api.exportDiagnostics());
    if (path) toast(t("Saved"));
  } catch (e) {
    showError(e as Error);
  }
}

function openConfigDir() {
  void import("../api")
    .then((m) => m.api.openConfigDir())
    .catch(showError);
}

const menuDefs = computed<MenuDef[]>(() => [
  {
    name: "file",
    label: t("File"),
    accel: "F",
    items: [
      { label: t("New Repository"), shortcut: "Ctrl+N", action: () => (store.ui.newRepo = true) },
      { label: t("Clone Repository"), shortcut: "Ctrl+Shift+C", action: () => (store.ui.clone = true) },
      { label: t("Add Local Repository"), action: addLocalRepoAction },
      { separator: true },
      { label: t("Settings"), shortcut: "Ctrl+,", action: () => (store.ui.settings = true) },
      { label: t("Exit"), action: () => window.close() },
    ],
  },
  {
    name: "view",
    label: t("View"),
    accel: "V",
    items: [
      { label: t("Toggle Sidebar"), shortcut: "Ctrl+B", action: () => (store.sidebarVisible = !store.sidebarVisible) },
      { separator: true },
      {
        label: t("Show Ignored Files"),
        checked: store.config?.showIgnored ?? false,
        action: () => void persistConfig({ showIgnored: !store.config?.showIgnored }),
      },
      { separator: true },
      {
        label: t("Auto Refresh"),
        checked: store.config?.autoRefresh ?? true,
        action: () => void persistConfig({ autoRefresh: !store.config?.autoRefresh }),
      },
    ],
  },
  {
    name: "tools",
    label: t("Tools"),
    accel: "T",
    items: [
      { label: t("Open Config Directory"), action: openConfigDir },
      { label: t("Export Diagnostic Logs"), action: exportDiagnostics },
    ],
  },
  {
    name: "help",
    label: t("Help"),
    accel: "H",
    items: [
      { label: t("Keyboard Shortcuts"), action: () => (store.ui.shortcuts = true) },
      { label: t("About HiGit"), action: () => (store.ui.about = true) },
    ],
  },
]);

// 点击菜单栏以外任意位置 / 按 Esc 时收起弹出菜单；Windows/Linux 响应 Alt+助记键
const menubarEl = ref<HTMLElement | null>(null);

function onDocPointerDown(e: MouseEvent) {
  if (openMenu.value && menubarEl.value && !menubarEl.value.contains(e.target as Node)) {
    openMenu.value = null;
  }
}

function onDocKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") openMenu.value = null;
}

let altAlone = false;

function onMenubarKeydown(e: KeyboardEvent) {
  if (isMac) return;
  if (e.key === "Alt") {
    e.preventDefault();
    altAlone = true;
    return;
  }
  if (!e.altKey || e.ctrlKey || e.metaKey) {
    altAlone = false;
    return;
  }
  altAlone = false;
  const def = menuDefs.value.find((m) => m.accel === e.key.toUpperCase());
  if (def) {
    e.preventDefault();
    openMenu.value = openMenu.value === def.name ? null : def.name;
  }
}

function onMenubarKeyup(e: KeyboardEvent) {
  if (isMac || e.key !== "Alt") return;
  // 单独按 Alt：切换菜单栏激活状态（打开第一个菜单 / 收起）
  if (altAlone) {
    e.preventDefault();
    openMenu.value = openMenu.value ? null : menuDefs.value[0].name;
  }
  altAlone = false;
}

onMounted(() => {
  document.addEventListener("mousedown", onDocPointerDown, true);
  document.addEventListener("keydown", onDocKeydown, true);
  document.addEventListener("keydown", onMenubarKeydown, true);
  document.addEventListener("keyup", onMenubarKeyup, true);
});

onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onDocPointerDown, true);
  document.removeEventListener("keydown", onDocKeydown, true);
  document.removeEventListener("keydown", onMenubarKeydown, true);
  document.removeEventListener("keyup", onMenubarKeyup, true);
});

/** 菜单标签：中文追加 “(F)”；英文在首字母下标注记（均为首字母） */
function labelHtml(def: MenuDef): string {
  if (isMac) return escapeHtml(def.label);
  if (isZh.value) return `${escapeHtml(def.label)}<span class="accel">(${def.accel})</span>`;
  return `<span class="accel">${escapeHtml(def.accel)}</span>${escapeHtml(def.label.slice(1))}`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
</script>

<template>
  <div ref="menubarEl" class="menubar">
    <div v-for="def in menuDefs" :key="def.name" class="menu-root">
      <button
        class="menu-label"
        :class="{ open: openMenu === def.name }"
        @click="openMenu = openMenu === def.name ? null : def.name"
        @mouseenter="openMenu !== null && (openMenu = def.name)"
        v-html="labelHtml(def)"
      />
      <div v-if="openMenu === def.name" class="menu-pop">
        <template v-for="(item, i) in def.items" :key="i">
          <div v-if="item.separator" class="menu-sep" />
          <div v-else-if="item.children" class="menu-item has-children">
            <span class="ellipsis">{{ item.label }}</span>
            <svg width="12" height="12" viewBox="0 0 24 24" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round">
              <path d="M9 5.5l6.5 6.5L9 18.5" />
            </svg>
            <div class="menu-sub">
              <template v-for="(sub, j) in item.children()" :key="j">
                <div v-if="sub.separator" class="menu-sep" />
                <button v-else class="menu-item" :disabled="sub.disabled" @click="sub.action?.(); openMenu = null">
                  <span class="menu-check">{{ sub.checked ? "✓" : "" }}</span>
                  <span class="ellipsis">{{ sub.label }}</span>
                </button>
              </template>
            </div>
          </div>
          <button
            v-else
            class="menu-item"
            :disabled="item.disabled"
            @click="item.action?.(); openMenu = null"
          >
            <span class="menu-check">{{ item.checked ? "✓" : "" }}</span>
            <span class="ellipsis">{{ item.label }}</span>
            <span v-if="item.shortcut" class="menu-shortcut">{{ item.shortcut }}</span>
          </button>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.menubar {
  display: flex;
  background: var(--bg-menubar);
  padding: 0 6px;
  flex: none;
  position: relative;
  z-index: 900;
}

.menu-root {
  position: relative;
}

.menu-label {
  border: none;
  background: none;
  padding: 6px 12px;
  font-size: 13px;
  color: var(--text-primary);
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.menu-label:hover,
.menu-label.open {
  background: var(--bg-hover);
}

.accel {
  text-decoration: underline;
  text-underline-offset: 2px;
}

.menu-pop {
  position: absolute;
  top: calc(100% + 2px);
  left: 0;
  min-width: 240px;
  background: var(--bg-dialog);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-menu);
  padding: 4px;
  z-index: 950;
  display: flex;
  flex-direction: column;
}

.menu-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  border: none;
  background: none;
  color: var(--text-primary);
  font-size: 13px;
  padding: 7px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  text-align: left;
  position: relative;
}

.menu-item:hover:not(:disabled) {
  background: var(--bg-hover);
}

.menu-item:disabled {
  color: var(--text-disabled);
  cursor: default;
}

.menu-check {
  width: 14px;
  flex: none;
  font-size: 12px;
}

.menu-shortcut {
  margin-left: auto;
  color: var(--text-tertiary);
  font-size: 12px;
  padding-left: 24px;
}

.menu-sep {
  height: 1px;
  background: var(--divider);
  margin: 4px 8px;
}

.has-children > .menu-sub {
  display: none;
  position: absolute;
  left: calc(100% - 4px);
  top: -5px;
  min-width: 220px;
  background: var(--bg-dialog);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-menu);
  padding: 4px;
  flex-direction: column;
  z-index: 960;
}

.has-children:hover > .menu-sub {
  display: flex;
}
</style>
