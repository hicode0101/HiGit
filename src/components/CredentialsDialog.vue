<script setup lang="ts">
/** 凭证对话框（FR-5.5 / FR-5.6）：HTTPS 用户名+密码 / SSH 密钥+口令，可记住 */
import { ref } from "vue";
import { store, showError, toast } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import { retrySyncWithCreds } from "../tasks";
import DialogBase from "./DialogBase.vue";
import PasswordInput from "./PasswordInput.vue";

const emit = defineEmits<{ close: [] }>();

const payload = () => store.ui.credentials!;
const username = ref("");
const secret = ref("");
const keyPath = ref("");
const passphrase = ref("");
const remember = ref(true);

async function chooseKey() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({ title: t("SSH key file") });
  if (file && typeof file === "string") keyPath.value = file;
}

async function submit() {
  const p = payload();
  const creds =
    p.kind === "http"
      ? {
          kind: "http" as const,
          username: username.value || p.username,
          password: secret.value,
        }
      : {
          kind: "ssh" as const,
          username: username.value || p.username || "git",
          keyPath: keyPath.value,
          passphrase: passphrase.value,
        };
  if (p.kind === "http" && (!creds.username || !secret.value)) {
    store.ui.error = { message: t("Username and password are required"), raw: "" };
    return;
  }
  // "记住" → 系统凭证管理器（FR-5.5）
  if (remember.value) {
    try {
      await api.saveCredential(p.host, creds);
    } catch {
      /* 尽力而为 */
    }
  }
  store.ui.credentials = null;
  if (p.retry) {
    await retrySyncWithCreds(p.retry.kind, p.retry.branch, creds);
  } else {
    p.onCreds?.(creds, remember.value);
  }
}
</script>

<template>
  <DialogBase :title="t('Sign in required')" :width="480" :close-on-mask="false" @close="emit('close')">
    <div class="hint">
      {{ t("Credentials are required to access") }} <b>{{ payload().host }}</b>
    </div>

    <template v-if="payload().kind === 'http'">
      <div class="form-row">
        <label>{{ t("Username") }}</label>
        <input v-model="username" :placeholder="payload().username" />
      </div>
      <div class="form-row">
        <label>{{ t("Password / Token") }}</label>
        <PasswordInput v-model="secret" @keydown.enter="submit" />
      </div>
    </template>
    <template v-else>
      <div class="form-row">
        <label>{{ t("SSH key file") }} — {{ t("Empty = try ssh-agent and default keys") }}</label>
        <div class="form-inline">
          <input v-model="keyPath" />
          <button class="btn" @click="chooseKey">{{ t("Choose…") }}</button>
        </div>
      </div>
      <div class="form-row">
        <label>{{ t("Passphrase") }}</label>
        <PasswordInput v-model="passphrase" @keydown.enter="submit" />
      </div>
    </template>

    <label class="check">
      <input v-model="remember" type="checkbox" />
      <span>{{ t("Remember credentials in system credential manager") }}</span>
    </label>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" @click="submit">{{ t("Sign in") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.hint {
  font-size: 13px;
  margin-bottom: 14px;
  color: var(--text-secondary);
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 5px;
  margin-bottom: 12px;
}

.form-row label {
  font-size: 12px;
  color: var(--text-secondary);
}

.check {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  cursor: pointer;
}
</style>
