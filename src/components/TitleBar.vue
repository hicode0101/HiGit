<script setup lang="ts">
/** 自绘标题栏（FR §5.1）：Logo + 窗口控制，可拖拽 */
import { onMounted, onUnmounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import appIcon from "../assets/icon.png";

const win = getCurrentWindow();
const maximized = ref(false);

let unlisten: (() => void) | null = null;
onMounted(async () => {
  maximized.value = await win.isMaximized();
  unlisten = await win.onResized(async () => {
    maximized.value = await win.isMaximized();
  });
});
onUnmounted(() => unlisten?.());

async function toggleMaximize() {
  await win.toggleMaximize();
}
</script>

<template>
  <div class="titlebar" data-tauri-drag-region>
    <div class="app-brand" data-tauri-drag-region>
      <img :src="appIcon" class="app-logo" alt="HiGit" draggable="false" />
      <span class="app-name" data-tauri-drag-region>HiGit</span>
    </div>
    <div class="win-controls">
      <button class="win-btn" aria-label="minimize" @click="win.minimize()">
        <svg width="11" height="11" viewBox="0 0 11 11"><path d="M0.5 5.5h10" stroke="currentColor" /></svg>
      </button>
      <button class="win-btn" aria-label="maximize" @click="toggleMaximize">
        <svg v-if="!maximized" width="11" height="11" viewBox="0 0 11 11">
          <rect x="0.5" y="0.5" width="10" height="10" fill="none" stroke="currentColor" />
        </svg>
        <svg v-else width="11" height="11" viewBox="0 0 11 11">
          <rect x="0.5" y="2.5" width="8" height="8" fill="none" stroke="currentColor" />
          <path d="M2.5 2.5v-2h8v8h-2" fill="none" stroke="currentColor" />
        </svg>
      </button>
      <button class="win-btn close" aria-label="close" @click="win.close()">
        <svg width="11" height="11" viewBox="0 0 11 11"><path d="M0.5 0.5l10 10M10.5 0.5l-10 10" stroke="currentColor" /></svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.titlebar {
  height: 40px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: var(--bg-titlebar);
  border-bottom: 1px solid var(--divider);
  -webkit-app-region: drag;
  flex: none;
}

.app-brand {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-left: 14px;
}

.app-logo {
  width: 18px;
  height: 18px;
  user-select: none;
}

.app-name {
  font-size: 13px;
  font-weight: 600;
}

.win-controls {
  display: flex;
  height: 100%;
  -webkit-app-region: no-drag;
}

.win-btn {
  width: 46px;
  height: 100%;
  border: none;
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.win-btn:hover {
  background: var(--bg-hover);
}

.win-btn.close:hover {
  background: #c42b1c;
  color: #fff;
}
</style>
