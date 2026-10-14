<script setup lang="ts">
/** 设置对话框（FR-10）：语言 / 主题 / 开关 / Git 身份 / 网络代理 */
import { onMounted, ref } from "vue";
import { store, persistConfig, showError, toast } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

const emit = defineEmits<{ close: [] }>();

const globalIdentity = ref<{ name: string; email: string }>({ name: "", email: "" });

onMounted(async () => {
  try {
    const id = await api.getGlobalIdentity();
    globalIdentity.value = id ?? { name: "", email: "" };
  } catch {
    /* ignore */
  }
});

async function save() {
  try {
    if (globalIdentity.value.name.trim() && globalIdentity.value.email.trim()) {
      await api.setIdentity("global", { name: globalIdentity.value.name, email: globalIdentity.value.email });
    }
    await persistConfig();
    toast(t("Saved"));
    emit("close");
  } catch (e) {
    showError(e as Error);
  }
}
</script>

<template>
  <DialogBase :title="t('Settings')" :width="520" @close="emit('close')">
    <div class="sec">
      <div class="sec-title">{{ t("Language") }}</div>
      <div class="opt-row">
        <label class="radio"><input v-model="store.config!.language" type="radio" value="en" /> English</label>
        <label class="radio"><input v-model="store.config!.language" type="radio" value="zh-CN" /> 简体中文</label>
        <label class="radio"><input v-model="store.config!.language" type="radio" value="" /> {{ t("Follow system") }}</label>
      </div>

      <div class="sec-title">{{ t("Theme") }}</div>
      <div class="opt-row">
        <label class="radio"><input v-model="store.config!.theme" type="radio" value="light" /> {{ t("Light") }}</label>
        <label class="radio"><input v-model="store.config!.theme" type="radio" value="dark" /> {{ t("Dark") }}</label>
        <label class="radio"><input v-model="store.config!.theme" type="radio" value="system" /> {{ t("Follow system") }}</label>
      </div>

      <div class="sec-title">{{ t("View") }}</div>
      <label class="check"><input v-model="store.config!.autoRefresh" type="checkbox" /> {{ t("Auto Refresh") }}</label>
      <label class="check"><input v-model="store.config!.showIgnored" type="checkbox" /> {{ t("Show Ignored Files") }}</label>

      <div class="sec-title">{{ t("Git Identity") }}（{{ t("Apply globally (all repositories)") }}）</div>
      <div class="form-row">
        <label>{{ t("Your Name") }}</label>
        <input v-model="globalIdentity.name" />
      </div>
      <div class="form-row">
        <label>{{ t("Email") }}</label>
        <input v-model="globalIdentity.email" />
      </div>

      <div class="sec-title">{{ t("Network") }}</div>
      <div class="form-row">
        <label>{{ t("Proxy URL") }}</label>
        <input v-model="store.config!.proxy" placeholder="socks5://127.0.0.1:10808" />
        <span class="hint">{{ t("Applied to clone / fetch / push / pull. Leave empty for direct connection.") }}</span>
      </div>
    </div>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" @click="save">{{ t("Save") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.sec-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.4px;
  margin: 16px 0 8px;
}

.sec-title:first-child {
  margin-top: 0;
}

.opt-row {
  display: flex;
  gap: 20px;
  flex-wrap: wrap;
}

.radio,
.check {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 13px;
  cursor: pointer;
}

.check {
  margin-bottom: 8px;
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 5px;
  margin-bottom: 10px;
}

.form-row label {
  font-size: 12px;
  color: var(--text-secondary);
}

.hint {
  font-size: 11px;
  color: var(--text-tertiary);
}
</style>
