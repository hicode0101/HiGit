<script setup lang="ts">
/** 仓库设置对话框（FR-1.7 / FR-5.5 / FR-10.3）：别名 / 远端 URL / 认证 / Git 身份 */
import { ref } from "vue";
import { store, showError, toast, refreshRepos, refreshIdentity } from "../store";
import { api } from "../api";
import type { RepoEntryDto } from "../api/types";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

const emit = defineEmits<{ close: [] }>();

// 实时按 id 从后端取条目：前端持有的列表快照可能过期（同名仓库也各自唯一）
const entry = ref<RepoEntryDto | null>(null);

const tab = ref<0 | 1 | 2>(0);
const tabs = [t("Repo Info"), t("Auth"), t("Git Identity")];

const alias = ref("");
const url = ref("");
// 认证页（与克隆对话框一致）：HTTPS = 用户名 + 密码/Token + 记住凭证；SSH = 仅密钥文件
const authKind = ref<"http" | "ssh">("http");
const username = ref("");
const password = ref("");
const keyPath = ref("");
const rememberCred = ref(true);
// Git 身份页（与克隆对话框一致）：姓名 / 邮箱双列，两栏留空跟随全局
const idName = ref("");
const idEmail = ref("");
const loadedFor = ref("");

async function load() {
  const repoId = store.ui.repoSettingsId;
  if (!repoId || loadedFor.value === repoId) return;
  try {
    const e = await api.getRepoSettings(repoId);
    if (store.ui.repoSettingsId !== repoId) return; // 已关闭/切换目标，丢弃结果
    entry.value = e;
    loadedFor.value = repoId;
    // 仓库名 = 显示名：新建 / 克隆 / 添加后即为目录名，这里预填当前值（必填，不可为空）
    alias.value = e.alias ?? e.name;
    url.value = e.url ?? "";
    authKind.value = (e.auth?.kind as "http" | "ssh") ?? "http";
    username.value = e.auth?.username ?? "";
    keyPath.value = e.auth?.keyPath ?? "";
    idName.value = e.identity?.name ?? "";
    idEmail.value = e.identity?.email ?? "";
  } catch (err) {
    showError(err as Error);
  }
}
void load();

/** 从仓库远端 URL 提取 host（与后端 host_from_url 同规则），用于凭证入库 */
function hostFromUrl(u: string): string {
  const s = u.trim();
  if (s.startsWith("git@")) {
    const host = s.slice(4).split(":")[0];
    if (host) return host;
  }
  const idx = s.indexOf("://");
  if (idx >= 0) {
    let authority = s.slice(idx + 3).split("/")[0] ?? "";
    authority = authority.split("@").pop() ?? authority;
    const host = authority.split(":")[0] ?? "";
    if (host) return host;
  }
  return "";
}

async function chooseKey() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({ title: t("SSH key file") });
  if (file && typeof file === "string") keyPath.value = file;
}

async function save() {
  const e = entry.value;
  if (!e) return;
  if (!alias.value.trim()) {
    store.ui.error = { message: t("Repository name cannot be empty"), raw: "" };
    return;
  }
  const auth =
    authKind.value === "ssh"
      ? { kind: "ssh" as const, username: username.value.trim() || "git", keyPath: keyPath.value.trim() }
      : username.value.trim()
        ? { kind: "http" as const, username: username.value.trim(), keyPath: "" }
        : null;
  try {
    await api.saveRepoSettings({
      id: e.id,
      path: e.path,
      alias: alias.value.trim(),
      auth,
      url: url.value.trim(),
      identity:
        !idName.value.trim() && !idEmail.value.trim()
          ? null
          : { name: idName.value, email: idEmail.value },
    });
    // HTTPS 勾选"记住"且填了密码：写入系统凭证管理器（host 取自远端 URL）
    if (authKind.value === "http" && rememberCred.value && password.value && url.value.trim()) {
      const host = hostFromUrl(url.value);
      if (host) {
        await api
          .saveCredential(host, { kind: "http", username: username.value.trim(), password: password.value })
          .catch(() => undefined);
      }
    }
    toast(t("Saved"));
    await Promise.all([refreshRepos(), refreshIdentity()]);
    emit("close");
  } catch (err) {
    showError(err as Error);
  }
}
</script>

<template>
  <DialogBase :title="`${t('Repository Settings')} — ${entry?.name ?? ''}`" :width="560" @close="emit('close')">
    <div class="tabs">
      <button v-for="(name, i) in tabs" :key="name" class="tab-chip" :class="{ on: tab === i }" @click="tab = i as 0 | 1 | 2">
        {{ name }}
      </button>
    </div>

    <div v-show="tab === 0">
      <div class="form-row">
        <label>{{ t("Repo Name") }} — {{ t("Display name (required)") }}</label>
        <input v-model="alias" />
      </div>
      <div class="form-row">
        <label>{{ t("Remote repository URL") }} — {{ t("Push / pull target remote; leave empty to follow the repository git config") }}</label>
        <input v-model="url" placeholder="https://github.com/user/repo.git" />
      </div>
    </div>

    <div v-show="tab === 1">
      <div class="form-row">
        <div class="kind-row">
          <label class="radio"><input v-model="authKind" type="radio" value="http" /> HTTPS</label>
          <label class="radio"><input v-model="authKind" type="radio" value="ssh" /> SSH</label>
        </div>
      </div>
      <div class="hint-line">{{ t("Applies to pull / push / fetch of this repository") }}</div>
      <template v-if="authKind === 'http'">
        <div class="form-row">
          <label>{{ t("Username") }}</label>
          <input v-model="username" placeholder="username" />
        </div>
        <div class="form-row">
          <label>{{ t("Password / Token") }}</label>
          <input v-model="password" type="password" />
        </div>
        <label class="check">
          <input v-model="rememberCred" type="checkbox" />
          <span>{{ t("Remember credentials in system credential manager") }}</span>
        </label>
      </template>
      <template v-else>
        <div class="form-row">
          <label>{{ t("SSH key file") }}</label>
          <div class="form-inline">
            <input v-model="keyPath" :placeholder="t('Empty = try ssh-agent and default keys')" />
            <button class="btn" @click="chooseKey">{{ t("Choose…") }}</button>
          </div>
        </div>
      </template>
    </div>

    <div v-show="tab === 2">
      <div class="identity-grid">
        <div class="form-row">
          <label>{{ t("Your Name") }}</label>
          <input v-model="idName" />
        </div>
        <div class="form-row">
          <label>{{ t("Email") }}</label>
          <input v-model="idEmail" />
        </div>
      </div>
      <div class="hint-line">
        {{ t("Used as the committer for this repository; leave both empty to follow the global identity") }}
      </div>
    </div>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!alias.trim()" @click="save">{{ t("Save") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.tabs {
  display: flex;
  gap: 8px;
  margin-bottom: 18px;
}

.tab-chip {
  border: 1px solid var(--border-color);
  background: var(--bg-card);
  color: var(--text-secondary);
  font-size: 12px;
  padding: 5px 14px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}

.tab-chip.on {
  background: var(--accent-subtle);
  color: var(--accent);
  border-color: var(--accent);
  font-weight: 600;
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

.kind-row {
  display: flex;
  gap: 18px;
}

.hint-line {
  color: var(--text-tertiary);
  font-size: 12px;
  margin: -8px 0 8px;
}

.identity-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
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
  margin-bottom: 12px;
}
</style>
