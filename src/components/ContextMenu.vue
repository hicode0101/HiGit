<script setup lang="ts">
/** 全局右键菜单浮层 + 全局点击关闭 */
import { onMounted, onUnmounted } from "vue";
import { ctxMenu, hideContextMenu, runCtxAction } from "../ctxmenu";
import IconGlyph from "./IconGlyph.vue";

function onDocClick(e: MouseEvent) {
  if (ctxMenu.visible && !(e.target as HTMLElement).closest(".ctx-menu")) {
    hideContextMenu();
  }
}

onMounted(() => {
  document.addEventListener("click", onDocClick, true);
  window.addEventListener("blur", hideContextMenu);
  window.addEventListener("resize", hideContextMenu);
});
onUnmounted(() => {
  document.removeEventListener("click", onDocClick, true);
});
</script>

<template>
  <Teleport to="body">
    <div
      v-if="ctxMenu.visible"
      class="ctx-menu"
      :style="{ left: ctxMenu.x + 'px', top: ctxMenu.y + 'px' }"
    >
      <template v-for="(item, i) in ctxMenu.items" :key="i">
        <div v-if="item.separator" class="ctx-sep" />
        <button
          v-else
          class="ctx-item"
          :class="{ danger: item.danger, disabled: item.disabled }"
          :disabled="item.disabled"
          @click="runCtxAction(item)"
        >
          <span class="ctx-icon"><IconGlyph v-if="item.icon" :name="item.icon" :size="15" /></span>
          <span class="ellipsis">{{ item.label }}</span>
        </button>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.ctx-menu {
  position: fixed;
  z-index: 4000;
  min-width: 180px;
  padding: 4px;
  background: var(--bg-dialog);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-menu);
  display: flex;
  flex-direction: column;
}

.ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 12px 7px 6px;
  border: none;
  background: none;
  color: var(--text-primary);
  font-size: 13px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  text-align: left;
  width: 100%;
}

.ctx-item:hover:not(.disabled) {
  background: var(--bg-hover);
}

.ctx-item.danger {
  color: var(--status-deleted);
}

.ctx-item.disabled {
  color: var(--text-disabled);
  cursor: default;
}

.ctx-icon {
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
}

.ctx-sep {
  height: 1px;
  background: var(--divider);
  margin: 4px 8px;
}
</style>
