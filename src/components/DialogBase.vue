<script setup lang="ts">
/** 通用对话框骨架：遮罩 + Fluent 卡片；标题栏左键可拖动位置，可选最大化与边框拖拽缩放 */
import { onBeforeUnmount, onMounted, ref } from "vue";
import { t } from "../i18n";

const props = withDefaults(
  defineProps<{
    title: string;
    width?: number;
    closeOnMask?: boolean;
    /** 标题栏显示最大化 / 还原按钮（双击标题栏同效） */
    maximizable?: boolean;
    /** 允许拖动边框调整大小 */
    resizable?: boolean;
    /** 标题栏 X 关闭按钮（门禁类对话框设为 false 强制用户完成输入） */
    closable?: boolean;
  }>(),
  { width: 560, closeOnMask: true, maximizable: false, resizable: false, closable: true },
);

const emit = defineEmits<{ close: [] }>();

const dlgEl = ref<HTMLElement | null>(null);
const maximized = ref(false);
const resizedOnce = ref(false);

onMounted(() => {
  // 宽度走一次性内联样式而非 :style 绑定：Vue patchStyle 每次重渲染都会重写绑定键，
  // 会覆盖拖拽/最大化期间直接写入的 calc() 宽度（高度/位置无绑定，直接样式不受影响）
  if (dlgEl.value) dlgEl.value.style.width = `${props.width}px`;
});

const RS_DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const;

let dragging = false;
let startX = 0;
let startY = 0;
let origX = 0;
let origY = 0;
let dlgW = 0;
let dlgH = 0;

function onTitleMousedown(e: MouseEvent) {
  if (e.button !== 0 || maximized.value) return;
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
  if (!dragging || maximized.value) return;
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

function onTitleDblclick(e: MouseEvent) {
  if (!props.maximizable) return;
  if ((e.target as HTMLElement).closest("button, input, textarea, select")) return;
  toggleMaximize();
}

// ---- 最大化 / 还原：直接样式覆写，还原时回到记录的值（空串 = 移除内联样式回退 CSS） ----
let savedStyle: Partial<CSSStyleDeclaration> | null = null;

function toggleMaximize() {
  const el = dlgEl.value;
  if (!el) return;
  if (!maximized.value) {
    savedStyle = {
      position: el.style.position,
      left: el.style.left,
      top: el.style.top,
      width: el.style.width,
      height: el.style.height,
      margin: el.style.margin,
      maxWidth: el.style.maxWidth,
      maxHeight: el.style.maxHeight,
    };
    el.style.position = "fixed";
    el.style.left = "12px";
    el.style.top = "12px";
    el.style.width = "calc(100vw - 24px)";
    el.style.height = "calc(100vh - 24px)";
    el.style.margin = "0";
    el.style.maxWidth = "none";
    el.style.maxHeight = "none";
    maximized.value = true;
  } else {
    const s = savedStyle;
    if (s) {
      el.style.position = s.position ?? "";
      el.style.left = s.left ?? "";
      el.style.top = s.top ?? "";
      el.style.width = s.width ?? "";
      el.style.height = s.height ?? "";
      el.style.margin = s.margin ?? "";
      el.style.maxWidth = s.maxWidth ?? "";
      el.style.maxHeight = s.maxHeight ?? "";
    }
    maximized.value = false;
  }
}

// ---- 边框拖拽缩放（八方向） ----
let resizing = false;
let rsDir = "";
let rs = { x: 0, y: 0, w: 0, h: 0, l: 0, t: 0 };

function onResizeStart(e: MouseEvent, dir: string) {
  if (e.button !== 0 || maximized.value) return;
  const el = dlgEl.value;
  if (!el) return;
  e.preventDefault();
  e.stopPropagation();
  const rect = el.getBoundingClientRect();
  // 与标题拖动一致：转为 fixed 定位并把尺寸落成显式宽高（sized 布局立即生效不塌陷），
  // 同时解除 76vh 高度上限
  el.style.position = "fixed";
  el.style.left = `${rect.left}px`;
  el.style.top = `${rect.top}px`;
  el.style.width = `${rect.width}px`;
  el.style.height = `${rect.height}px`;
  el.style.margin = "0";
  el.style.maxWidth = "none";
  el.style.maxHeight = "none";
  rsDir = dir;
  rs = { x: e.clientX, y: e.clientY, w: rect.width, h: rect.height, l: rect.left, t: rect.top };
  resizing = true;
  resizedOnce.value = true;
  document.body.style.userSelect = "none";
  window.addEventListener("mousemove", onResizeMove);
  window.addEventListener("mouseup", onResizeEnd);
}

function onResizeMove(e: MouseEvent) {
  if (!resizing) return;
  const el = dlgEl.value;
  if (!el) return;
  const MIN_W = 320;
  const MIN_H = 200;
  const dx = e.clientX - rs.x;
  const dy = e.clientY - rs.y;
  let l = rs.l;
  let t = rs.t;
  // 西/北向先夹住左/上边界（对侧边缘保持不动）
  if (rsDir.includes("w")) l = Math.min(Math.max(rs.l + dx, 8), rs.l + rs.w - MIN_W);
  if (rsDir.includes("n")) t = Math.min(Math.max(rs.t + dy, 8), rs.t + rs.h - MIN_H);
  let w = rsDir.includes("w") ? rs.l + rs.w - l : rs.w + (rsDir.includes("e") ? dx : 0);
  let h = rsDir.includes("n") ? rs.t + rs.h - t : rs.h + (rsDir.includes("s") ? dy : 0);
  // 不超出视口
  w = Math.min(w, window.innerWidth - l - 8);
  h = Math.min(h, window.innerHeight - t - 8);
  el.style.left = `${l}px`;
  el.style.top = `${t}px`;
  el.style.width = `${Math.max(w, MIN_W)}px`;
  el.style.height = `${Math.max(h, MIN_H)}px`;
}

function onResizeEnd() {
  resizing = false;
  document.body.style.userSelect = "";
  window.removeEventListener("mousemove", onResizeMove);
  window.removeEventListener("mouseup", onResizeEnd);
}

onBeforeUnmount(() => {
  dragging = false;
  resizing = false;
  document.body.style.userSelect = "";
  window.removeEventListener("mousemove", onDragMove);
  window.removeEventListener("mouseup", onDragEnd);
  window.removeEventListener("mousemove", onResizeMove);
  window.removeEventListener("mouseup", onResizeEnd);
});
</script>

<template>
  <Teleport to="body">
    <div class="dlg-mask" @mousedown.self="closeOnMask && emit('close')">
      <div ref="dlgEl" class="dlg" :class="{ sized: maximized || resizedOnce }">
        <div class="dlg-title" @mousedown="onTitleMousedown" @dblclick="onTitleDblclick">
          <span class="ellipsis">{{ title }}</span>
          <span class="dlg-title-btns">
            <button
              v-if="maximizable"
              class="dlg-close"
              :title="maximized ? t('Restore') : t('Maximize')"
              @click="toggleMaximize"
            >
              <svg v-if="!maximized" width="12" height="12" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="square">
                <rect x="5" y="5" width="14" height="14" />
              </svg>
              <svg v-else width="12" height="12" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="square">
                <rect x="4" y="10" width="10" height="10" />
                <path d="M9 6V4h11v11h-2" />
              </svg>
            </button>
            <button v-if="closable" class="dlg-close" aria-label="close" @click="emit('close')">
              <svg width="12" height="12" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round">
                <path d="M5 5l14 14M19 5L5 19" />
              </svg>
            </button>
          </span>
        </div>
        <div class="dlg-body" :class="{ fill: maximizable || resizable }">
          <slot />
        </div>
        <div v-if="$slots.footer" class="dlg-footer">
          <slot name="footer" />
        </div>
        <template v-if="resizable && !maximized">
          <div
            v-for="d in RS_DIRS"
            :key="d"
            class="rs"
            :class="`rs-${d}`"
            @mousedown="onResizeStart($event, d)"
          />
        </template>
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
  /* relative 让绝对定位的缩放手柄锚定对话框自身（未拖动/缩放前不是 fixed） */
  position: relative;
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

.dlg-title-btns {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: none;
}

/* 八方向缩放手柄：覆盖在边框内侧，角落加大热区便于抓取 */
.rs {
  position: absolute;
  z-index: 20;
}

.rs-n { top: 0; left: 14px; right: 14px; height: 5px; cursor: ns-resize; }
.rs-s { bottom: 0; left: 14px; right: 14px; height: 6px; cursor: ns-resize; }
.rs-e { right: 0; top: 14px; bottom: 14px; width: 6px; cursor: ew-resize; }
.rs-w { left: 0; top: 14px; bottom: 14px; width: 6px; cursor: ew-resize; }
.rs-ne { top: 0; right: 0; width: 16px; height: 16px; cursor: nesw-resize; }
.rs-nw { top: 0; left: 0; width: 16px; height: 16px; cursor: nwse-resize; }
.rs-se { bottom: 0; right: 0; width: 16px; height: 16px; cursor: nwse-resize; }
.rs-sw { bottom: 0; left: 0; width: 16px; height: 16px; cursor: nesw-resize; }

/* 可缩放/最大化对话框：body 变纵向 flex；一旦有了显式高度（sized）即撑满剩余空间，
   由 slot 内容自行决定内部滚动区 */
.dlg-body.fill {
  display: flex;
  flex-direction: column;
}

.dlg.sized .dlg-body.fill {
  flex: 1 1 0;
  min-height: 0;
  overflow: hidden;
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
