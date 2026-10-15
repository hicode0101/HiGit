<script setup lang="ts">
/** 设置对话框（FR-10）：语言 / 主题 / 开关 / Git 身份 / 网络代理 */
import { computed, onMounted, ref } from "vue";
import { store, persistConfig, showError, toast } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

const emit = defineEmits<{ close: [] }>();

/** 开启代理时的默认地址（引擎只对 HTTP 代理稳定可用，socks 不建议） */
const DEFAULT_PROXY = "http://127.0.0.1:7890";
/** 邮箱格式：非空白@非空白.非空白（宽松校验，避免误拒合法地址） */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** 填的是 SOCKS 地址时给出警示：引擎经系统组件处理 socks，实测与常见代理软件不兼容 */
const socksHint = computed(
  () =>
    !!store.config?.proxyEnabled &&
    store.config.proxy.trim().toLowerCase().startsWith("socks"),
);

const globalIdentity = ref<{ name: string; email: string }>({ name: "", email: "" });

onMounted(async () => {
  try {
    const id = await api.getGlobalIdentity();
    globalIdentity.value = id ?? { name: "", email: "" };
  } catch {
    /* ignore */
  }
});

/** 打开代理开关时，地址为空则预填默认地址 */
function onProxyToggle() {
  if (store.config!.proxyEnabled && !store.config!.proxy.trim()) {
    store.config!.proxy = DEFAULT_PROXY;
  }
}

async function save() {
  const name = globalIdentity.value.name.trim();
  const email = globalIdentity.value.email.trim();
  if (!name || !email) {
    store.ui.error = { message: t("Name and email are required"), raw: "" };
    return;
  }
  if (!EMAIL_RE.test(email)) {
    store.ui.error = { message: t("Invalid email format"), raw: "" };
    return;
  }
  try {
    await api.setIdentity("global", { name, email });
    if (store.config!.proxyEnabled && !store.config!.proxy.trim()) {
      store.config!.proxy = DEFAULT_PROXY;
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
      <label class="check">
        <input v-model="store.config!.proxyEnabled" type="checkbox" @change="onProxyToggle" />
        {{ t("Enable proxy") }}
      </label>
      <div class="form-row">
        <label>{{ t("Proxy URL") }}</label>
        <input
          v-model="store.config!.proxy"
          :disabled="!store.config!.proxyEnabled"
          placeholder="http://127.0.0.1:7890"
        />
        <span class="hint">{{ t("Applied to clone / fetch / push / pull when enabled.") }}</span>
        <span v-if="socksHint" class="hint warn-hint">
          {{ t("SOCKS proxies are unreliable with the built-in engine (often timeouts). Prefer an HTTP proxy address, e.g. http://127.0.0.1:7890.") }}
        </span>
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

.form-row input:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.hint {
  font-size: 11px;
  color: var(--text-tertiary);
}

.warn-hint {
  color: var(--status-modified);
}
</style>
