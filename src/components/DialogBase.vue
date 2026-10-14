<script setup lang="ts">
/** 通用对话框骨架：遮罩 + Fluent 卡片；标题栏左键可拖动位置 */
import { onBeforeUnmount, ref } from "vue";

withDefaults(
  defineProps<{
    title: string;
    width?: number;
    closeOnMask?: boolean;
  }>(),
  { width: 560, closeOnMask: true },
);

const emit = defineEmits<{ close: [] }>();

const dlgEl = ref<HTMLElement | null>(null);
let dragging = false;
let startX = 0;
let startY = 0;
let origX = 0;
let origY = 0;
let dlgW = 0;
let dlgH = 0;

function onTitleMousedown(e: MouseEvent) {
  if (e.button !== 0) return;
  const el = dlgEl.value;
  if (!el) return;
  // 标题栏内的按钮（如关闭）不触发拖动
  if ((e.target as HTMLElement).closest("button, input, textarea, select")) return;
  const rect = el.getBoundingClientRect();
  // 首次拖动时从 flex 居中改为 fixed 定位，之后按坐标自由移动
  el.style.position = "fixed";
  el.style.left = `${rect.left}px`;
  el.style.top = `${rect.top}px`;
  el.style.margin = "0";
  dlgW = rect.width;
  dlgH = rect.height;
  startX = e.clientX;
  startY = e.clientY;
  origX = rect.left;
  origY = rect.top;
  dragging = true;
  window.addEventListener("mousemove", onDragMove);
  window.addEventListener("mouseup", onDragEnd);
}

function onDragMove(e: MouseEvent) {
  if (!dragging) return;
  const el = dlgEl.value;
  if (!el) return;
  // 限制在窗口可视范围内
  const maxX = Math.max(8, window.innerWidth - dlgW - 8);
  const maxY = Math.max(8, window.innerHeight - dlgH - 8);
  const x = Math.min(Math.max(origX + e.clientX - startX, 8), maxX);
  const y = Math.min(Math.max(origY + e.clientY - startY, 8), maxY);
  el.style.left = `${x}px`;
  el.style.top = `${y}px`;
}

function onDragEnd() {
  dragging = false;
  window.removeEventListener("mousemove", onDragMove);
  window.removeEventListener("mouseup", onDragEnd);
}

onBeforeUnmount(() => {
  dragging = false;
  window.removeEventListener("mousemove", onDragMove);
  window.removeEventListener("mouseup", onDragEnd);
});
</script>

<template>
  <Teleport to="body">
    <div class="dlg-mask" @mousedown.self="closeOnMask && emit('close')">
      <div ref="dlgEl" class="dlg" :style="{ width: width + 'px' }">
        <div class="dlg-title" @mousedown="onTitleMousedown">
          <span class="ellipsis">{{ title }}</span>
          <button class="dlg-close" aria-label="close" @click="emit('close')">
            <svg width="12" height="12" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round">
              <path d="M5 5l14 14M19 5L5 19" />
            </svg>
          </button>
        </div>
        <div class="dlg-body">
          <slot />
        </div>
        <div v-if="$slots.footer" class="dlg-footer">
          <slot name="footer" />
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.dlg-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.35);
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 12vh;
}

.dlg {
  background: var(--bg-dialog);
  border-radius: var(--radius-lg);
  border: 1px solid var(--border-color);
  box-shadow: var(--shadow-dialog);
  max-width: calc(100vw - 48px);
  max-height: 76vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.dlg-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 24px 10px;
  font-size: 18px;
  font-weight: 600;
  user-select: none;
  cursor: default;
}

.dlg-close {
  border: none;
  background: none;
  color: var(--text-secondary);
  cursor: pointer;
  width: 28px;
  height: 28px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.dlg-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.dlg-body {
  padding: 4px 24px 16px;
  overflow-y: auto;
}

.dlg-footer {
  padding: 14px 24px;
  border-top: 1px solid var(--divider);
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}
</style>
