<script setup lang="ts">
/** 确认对话框（FR-11.4 破坏性操作二次确认） + 输入对话框 */
import { nextTick, ref, watch } from "vue";
import { store } from "../store";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

function confirmResolve(ok: boolean) {
  store.confirm.resolve?.(ok);
  store.confirm.resolve = null;
  store.confirm.visible = false;
}

const inputValue = ref("");
const inputEl = ref<HTMLInputElement | null>(null);

watch(
  () => store.input.visible,
  async (v) => {
    if (v) {
      inputValue.value = store.input.value;
      await nextTick();
      inputEl.value?.focus();
      inputEl.value?.select();
    }
  },
);

function inputResolve(v: string | null) {
  store.input.resolve?.(v);
  store.input.resolve = null;
  store.input.visible = false;
}
</script>

<template>
  <DialogBase
    :title="store.confirm.title"
    :width="420"
    :close-on-mask="false"
    v-if="store.confirm.visible"
    @close="confirmResolve(false)"
  >
    <div class="confirm-msg">{{ store.confirm.message }}</div>
    <template #footer>
      <button class="btn" @click="confirmResolve(false)">{{ t("Cancel") }}</button>
      <button class="btn" :class="store.confirm.danger ? 'danger' : 'primary'" @click="confirmResolve(true)">
        {{ store.confirm.confirmLabel }}
      </button>
    </template>
  </DialogBase>

  <DialogBase
    :title="store.input.title"
    :width="420"
    :close-on-mask="false"
    v-if="store.input.visible"
    @close="inputResolve(null)"
  >
    <div class="form-row">
      <label>{{ store.input.label }}</label>
      <input ref="inputEl" v-model="inputValue" @keydown.enter="inputResolve(inputValue)" />
    </div>
    <template #footer>
      <button class="btn" @click="inputResolve(null)">{{ t("Cancel") }}</button>
      <button class="btn primary" @click="inputResolve(inputValue)">{{ t("OK") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.confirm-msg {
  font-size: 13px;
  line-height: 1.7;
  word-break: break-word;
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.form-row label {
  font-size: 12px;
  color: var(--text-secondary);
}
</style>
