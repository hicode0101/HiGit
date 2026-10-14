<script setup lang="ts">
/** 轻提示（FR-11.3）：状态栏上方，4 秒后自动消退 */
import { store } from "../store";
</script>

<template>
  <div class="toasts">
    <TransitionGroup name="toast">
      <div v-for="tt in store.toasts" :key="tt.id" class="toast">{{ tt.text }}</div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toasts {
  position: fixed;
  bottom: 46px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 3000;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  pointer-events: none;
}

.toast {
  background: var(--bg-dialog);
  color: var(--text-primary);
  border: 1px solid var(--border-color);
  box-shadow: var(--shadow-menu);
  border-radius: var(--radius-md);
  padding: 9px 18px;
  font-size: 13px;
  max-width: 70vw;
}

.toast-enter-active,
.toast-leave-active {
  transition: all 0.25s ease;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(8px);
}
</style>
