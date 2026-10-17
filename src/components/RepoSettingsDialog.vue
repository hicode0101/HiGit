<script setup lang="ts">
/** 仓库设置对话框（FR-1.7 / FR-5.5 / FR-10.3）：别名 / 远端 URL / 认证 / Git 身份 */
import { computed, ref } from "vue";
import { store, showError, toast, refreshRepos, refreshIdentity } from "../store";
import { api } from "../api";
import type { RepoEntryDto } from "../api/types";
import { t } from "../i18n";
import { detectAuthFromUrl } from "../urlAuth";
import { isValidEmail } from "../validate";
import DialogBase from "./DialogBase.vue";
import PasswordInput from "./PasswordInput.vue";

const emit = defineEmits<{ close: [] }>();

// 实时按 id 从后端取条目：前端持有的列表快照可能过期（同名仓库也各自唯一）
const entry = ref<RepoEntryDto | null>(null);

const tab = ref<0 | 1 | 2>(0);
// computed 包裹：页签文案跟随语言切换实时刷新
const tabs = computed(() => [t("Repo Info"), t("Auth"), t("Git Identity")]);

const alias = ref("");
const url = ref("");
// 认证页（与克隆对话框一致）：HTTPS = 用户名 + 密码/Token + 记住凭证；SSH = 仅密钥文件
const authKind = ref<"http" | "ssh">("http");
const username = ref("");
const password = ref("");
const keyPath = ref("");
const rememberCred = ref(true);
/** 系统凭证管理器中是否已存有该仓库的 HTTPS 凭证（回显提示，密码本身不回显） */
const credSaved = ref(false);
// Git 身份页（与克隆对话框一致）：姓名 / 邮箱必填，留空的字段回退全局配置，两栏全空 = 跟随全局
const idName = ref("");
const idEmail = ref("");
const loadedFor = ref("");
const globalId = ref<{ name: string; email: string } | null>(null);

/** 邮箱非空但格式非法：禁止保存并提示（留空 = 清除 / 跟随全局，允许） */
const idEmailInvalid = computed(() => idEmail.value.trim() !== "" && !isValidEmail(idEmail.value));

/** 输入留空的字段回退全局配置；解析出完整姓名邮箱才允许保存 */
const resolvedName = computed(() => idName.value.trim() || globalId.value?.name || "");
const resolvedEmail = computed(() => idEmail.value.trim() || globalId.value?.email || "");
const identityComplete = computed(() => !!resolvedName.value && !!resolvedEmail.value);

/** 认证必填：HTTPS 需用户名 + 密码 / Token（已存凭证可沿用，密码留空 = 保持不变）；SSH 需密钥文件 */
const authComplete = computed(() =>
  authKind.value === "ssh"
    ? !!keyPath.value.trim()
    : (!!username.value.trim() && !!password.value) || credSaved.value,
);

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
    // 已保存凭证徽章（密码属机密不回显，仅提示存在与否）
    credSaved.value = await api.repoCredentialSaved(repoId).catch(() => false);
    // 全局身份：输入留空字段的回退来源（输入框下方提示将采用的全局值）
    if (!globalId.value) {
      globalId.value = await api.getGlobalIdentity().catch(() => null);
    }
  } catch (err) {
    showError(err as Error);
  }
}
void load();

async function chooseKey() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({ title: t("SSH key file") });
  if (file && typeof file === "string") keyPath.value = file;
}

// 修改远端 URL 时按格式自动切换认证方式（认证页随之联动）：
// https://host/用户名/... → HTTPS（用户名留空时填 URL 里的用户名段）；
// git@host:path / ssh://user@host/... → SSH（用户名取 @ 前的值，缺省 git）
function onUrlInput() {
  const d = detectAuthFromUrl(url.value);
  if (!d) return;
  authKind.value = d.kind;
  if (d.kind === "ssh") {
    username.value = d.username || "git";
  } else if (!username.value.trim() && d.username) {
    username.value = d.username;
  }
}

async function save() {
  const e = entry.value;
  if (!e) return;
  if (!alias.value.trim()) {
    store.ui.error = { message: t("Repository name cannot be empty"), raw: "" };
    return;
  }
  if (idEmailInvalid.value) {
    store.ui.error = { message: t("Invalid email format"), raw: "" };
    return;
  }
  // 姓名 / 邮箱必填：留空字段回退全局配置，两边都解析不出则拒绝保存
  if (!identityComplete.value) {
    store.ui.error = {
      message: t("Global identity is not configured. Fill in the name and email, or set them in Settings first."),
      raw: "",
    };
    return;
  }
  // 认证必填：HTTPS 用户名 + 密码 / Token（已存凭证可沿用）；SSH 密钥文件
  if (!authComplete.value) {
    store.ui.error = {
      message:
        authKind.value === "ssh"
          ? t("SSH authentication requires a key file.")
          : t("HTTPS authentication requires a username and password / token."),
      raw: "",
    };
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
          ? null // 两栏全空 = 跟随全局（运行时按全局身份回退），不落具体值
          : { name: resolvedName.value, email: resolvedEmail.value },
      // HTTPS 凭证随设置一并提交，由后端写入系统凭证管理器（host 解析含 git 配置回退）；
      // 留空 = 保持已存凭证不变；保存失败会在此抛错提示，不再静默
      password: authKind.value === "http" ? password.value : undefined,
      rememberCred: rememberCred.value,
    });
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
        <input v-model="url" placeholder="https://github.com/user/repo.git" @input="onUrlInput" />
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
          <PasswordInput v-model="password" placeholder="ghp_…" />
          <span v-if="credSaved" class="hint">
            ✓ {{ t("Saved in system credential manager. Leave empty to keep it.") }}
          </span>
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
            <input v-model="keyPath" />
            <button class="btn" @click="chooseKey">{{ t("Choose…") }}</button>
          </div>
        </div>
      </template>
      <div v-if="!authComplete" class="hint-line invalid-hint">
        {{
          authKind === "ssh"
            ? t("SSH authentication requires a key file.")
            : t("HTTPS authentication requires a username and password / token.")
        }}
      </div>
    </div>

    <div v-show="tab === 2">
      <div class="identity-grid">
        <div class="form-row">
          <label>{{ t("Your Name") }}</label>
          <input v-model="idName" />
          <span v-if="!idName.trim() && globalId?.name" class="fallback-hint">
            {{ t("Leave empty to use the global identity: {0}", { 0: globalId.name }) }}
          </span>
        </div>
        <div class="form-row">
          <label>{{ t("Email") }}</label>
          <input v-model="idEmail" />
          <span v-if="idEmailInvalid" class="invalid-hint">{{ t("Invalid email format") }}</span>
          <span v-else-if="!idEmail.trim() && globalId?.email" class="fallback-hint">
            {{ t("Leave empty to use the global identity: {0}", { 0: globalId.email }) }}
          </span>
        </div>
      </div>
      <div class="hint-line">
        {{ t("Used as the committer for this repository; leave both empty to follow the global identity") }}
      </div>
      <div v-if="!identityComplete" class="hint-line invalid-hint">
        {{ t("Global identity is not configured. Fill in the name and email, or set them in Settings first.") }}
      </div>
    </div>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button
        class="btn primary"
        :disabled="!alias.trim() || idEmailInvalid || !identityComplete || !authComplete"
        @click="save"
      >
        {{ t("Save") }}
      </button>
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

.invalid-hint {
  color: var(--status-modified);
  font-size: 11px;
}

.fallback-hint {
  color: var(--text-tertiary);
  font-size: 11px;
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
