<script setup lang="ts">
/** 克隆仓库对话框（图3 / FR-1.2）：仓库信息 / 认证 / Git 身份 三个标签页；
 * 开始后切换到进度 + 详细日志页（目标 / 认证 / 出网 / 阶段 / 结果，与同步对话框一致）+ 取消 */
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { store, showError, refreshRepos } from "../store";
import { api, onJobProgress, onSyncLog, replaySyncLogs } from "../api";
import type { JobDoneEvent } from "../api/types";
import { t } from "../i18n";
import {
  bindCloneDialogJob,
  finalizeCloneSuccess,
  registerCloneCreds,
  registerCloneDone,
  registerCloneSuccess,
} from "../tasks";
import { detectAuthFromUrl } from "../urlAuth";
import DialogBase from "./DialogBase.vue";
import PasswordInput from "./PasswordInput.vue";
import { joinNative } from "../path";

const emit = defineEmits<{ close: [] }>();

// 阶段：setup 配置 → running 执行中 → done 结束（日志保留可关闭）
const tab = ref<0 | 1 | 2>(0);
const url = ref("");
const dest = ref("");
const phase = ref<"setup" | "running" | "done">("setup");
const jobId = ref<number | null>(null);
const percent = ref(0);
const stage = ref("");
const cancelled = ref(false);
const logs = ref<{ level: string; text: string }[]>([]);
const logBox = ref<HTMLElement | null>(null);

// 认证页：HTTPS / SSH 单选，作用于本次克隆与之后该仓库的同步（FR-5）
const authKind = ref<"http" | "ssh">("http");
const username = ref("");
const password = ref("");
const keyPath = ref("");
const rememberCred = ref(true);

// Git 身份页：默认取全局身份，两栏留空时跟随全局（仓库级优先）
const identityName = ref("");
const identityEmail = ref("");

const tabs = computed(() => [t("Repo Info"), t("Auth"), t("Git Identity")]);

const repoNameFromUrl = computed(() => {
  const u = url.value.trim().replace(/\.git$/, "");
  const m = u.split(/[\\/:]/).filter(Boolean);
  return m.length ? m[m.length - 1] : "";
});

const destPath = computed(() => {
  if (!dest.value) return "";
  return joinNative(dest.value, repoNameFromUrl.value);
});

const canClone = computed(
  () => phase.value === "setup" && !!url.value.trim() && !!dest.value.trim(),
);

async function chooseDir() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const dir = await open({ directory: true, title: t("Destination directory") });
  if (dir && typeof dir === "string") dest.value = dir;
}

// 修改仓库 URL 时按格式自动切换认证方式（认证页随之联动）：
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

async function clone(credsOverride?: {
  kind: "http" | "ssh";
  username: string;
  password?: string;
  keyPath?: string;
  passphrase?: string;
}) {
  if (!canClone.value && !credsOverride) {
    store.ui.error = { message: t("Repository URL and destination are required"), raw: "" };
    return;
  }
  // 目标目录检查
  try {
    const st = await api.destStatus(destPath.value);
    if (st === "nonempty") {
      store.ui.error = { message: t("Destination directory already exists and is not empty"), raw: "" };
      return;
    }
  } catch {
    /* 后端兜底 */
  }
  cancelled.value = false;
  phase.value = "running";
  logs.value = [];
  percent.value = 0;
  stage.value = t("Cloning…");
  try {
    // HTTPS 填了用户名+密码才带凭证；SSH 始终带上（密钥留空 = ssh-agent / 默认密钥）
    const creds =
      credsOverride ??
      (authKind.value === "ssh"
        ? { kind: "ssh" as const, username: username.value || "git", keyPath: keyPath.value }
        : username.value && password.value
          ? { kind: "http" as const, username: username.value, password: password.value }
          : null);
    const jid = await api.cloneRepo({
      url: url.value.trim(),
      dest: destPath.value,
      creds,
      remember: rememberCred.value,
    });
    jobId.value = jid;
    bindCloneDialogJob(jid);
    replaySyncLogs(jid); // 补发早于 jobId 注册到达的开头日志（目标/认证方式/出网方式等）
  } catch (e) {
    phase.value = "done";
    showError(e as Error);
  }
}

// 凭证对话框提交后重试克隆（FR-1.2）
registerCloneCreds((creds) => {
  store.ui.credentials = null;
  void clone(creds);
});

// 克隆完成（结果在日志区展示）：成功 → 收尾打开仓库；认证失败 → 凭证框原地重试；
// 其他失败 → 对话框保留，失败行已在日志区
registerCloneDone((e: JobDoneEvent) => {
  if (e.jobId !== jobId.value) return;
  phase.value = "done";
  if (e.ok) {
    percent.value = 100;
    void finalizeCloneSuccess(e.repoPath);
    return;
  }
  if (e.error?.code === "auth-failed" && e.error.auth) {
    const auth = e.error.auth;
    store.ui.credentials = {
      kind: auth.kind,
      host: auth.host,
      username: auth.username,
      retry: null,
      onCreds: (creds) => {
        store.ui.credentials = null;
        void clone(creds);
      },
    };
  }
});

async function cancel() {
  cancelled.value = true;
  if (jobId.value) await api.cancelJob(jobId.value).catch(() => undefined);
}

// 进度事件由 tasks.ts 统一监听；这里同步进度条与详细日志
let unlisten: (() => void) | null = null;
let unlistenLog: (() => void) | null = null;
onMounted(async () => {
  // Git 身份默认取全局设置中的姓名 / 邮箱（仓库级设置优先于全局）
  try {
    const gi = await api.getGlobalIdentity();
    if (gi) {
      identityName.value = gi.name;
      identityEmail.value = gi.email;
    }
  } catch {
    /* ignore */
  }
  unlisten = await onJobProgress((e) => {
    if (jobId.value === e.jobId) {
      stage.value = `${t(e.stage)} ${e.done}/${e.total}`;
      percent.value = e.total > 0 ? Math.floor((e.done * 100) / e.total) : 0;
    }
  });
  // 详细日志：与同步对话框同一事件源（sync-log），按 job_id 过滤
  unlistenLog = await onSyncLog((e) => {
    if (e.jobId !== jobId.value) return;
    logs.value.push({ level: e.level, text: e.text });
    void nextTick(() => {
      if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight;
    });
  });
});
onUnmounted(() => {
  unlisten?.();
  unlistenLog?.();
  registerCloneDone(null);
});

function close() {
  if (phase.value === "running") void cancel();
  emit("close");
}

async function chooseKey() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({ title: t("SSH key file") });
  if (file && typeof file === "string") keyPath.value = file;
}

// 克隆成功后把克隆地址 / 认证 / 身份写入仓库条目（之后该仓库的拉取/推送/抓取沿用）
registerCloneSuccess(async (path) => {
  const auth =
    authKind.value === "ssh" ? { kind: "ssh" as const, username: username.value || "git", keyPath: keyPath.value } : null;
  const identity =
    identityName.value.trim() && identityEmail.value.trim()
      ? { name: identityName.value.trim(), email: identityEmail.value.trim() }
      : null;
  try {
    await api.saveRepoSettings({
      id: "",
      path,
      alias: null,
      auth,
      url: url.value.trim(), // 克隆地址存为该仓库的远端 URL
      identity,
    });
    void refreshRepos();
  } catch {
    /* ignore */
  }
});
</script>

<template>
  <DialogBase :title="t('Clone Repository')" :width="620" :close-on-mask="false" @close="close">
    <div class="tabs">
      <button v-for="(name, i) in tabs" :key="name" class="tab-chip" :class="{ on: tab === i }" @click="tab = i as 0 | 1 | 2">
        {{ name }}
      </button>
    </div>

    <div v-show="tab === 0 && phase === 'setup'">
      <div class="form-row">
        <label>{{ t("Repository URL") }}</label>
        <input v-model="url" :placeholder="'https://github.com/user/repo.git'" @input="onUrlInput" />
      </div>
      <div class="form-row">
        <label>{{ t("Destination directory") }}</label>
        <div class="form-inline">
          <input v-model="dest" :placeholder="'D:\\Code'" />
          <button class="btn" @click="chooseDir">{{ t("Choose…") }}</button>
        </div>
      </div>
      <div v-if="repoNameFromUrl" class="dest-preview">
        → {{ destPath }}
      </div>
    </div>

    <div v-show="tab === 1 && phase === 'setup'">
      <div class="form-row">
        <div class="kind-row">
          <label class="check-line"><input v-model="authKind" type="radio" value="http" /> HTTPS</label>
          <label class="check-line"><input v-model="authKind" type="radio" value="ssh" /> SSH</label>
        </div>
      </div>
      <div class="hint-line">{{ t("Applies to this clone and later pull / push / fetch of this repository") }}</div>
      <template v-if="authKind === 'http'">
        <div class="form-row">
          <label>{{ t("Username") }}</label>
          <input v-model="username" placeholder="username" />
        </div>
        <div class="form-row">
          <label>{{ t("Password / Token") }}</label>
          <PasswordInput v-model="password" />
        </div>
        <div class="form-row">
          <label class="check-line">
            <input v-model="rememberCred" type="checkbox" />
            <span>{{ t("Remember credentials in system credential manager") }}</span>
          </label>
        </div>
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

    <div v-show="tab === 2 && phase === 'setup'">
      <div class="identity-grid">
        <div class="form-row">
          <label>{{ t("Your Name") }}</label>
          <input v-model="identityName" />
        </div>
        <div class="form-row">
          <label>{{ t("Email") }}</label>
          <input v-model="identityEmail" />
        </div>
      </div>
      <div class="hint-line">
        {{ t("Used as the committer for this repository; leave both empty to follow the global identity") }}
      </div>
    </div>

    <!-- 执行中 / 结束：进度条 + 详细日志（与同步对话框一致，TortoiseGit 风格） -->
    <div v-if="phase !== 'setup'" class="progress-area">
      <div class="progress-title">{{ stage || t("Cloning…") }}</div>
      <div class="bar">
        <div
          class="bar-fill"
          :class="{ done: phase === 'done' && percent === 100 }"
          :style="{ width: percent + '%' }"
        />
      </div>
      <div ref="logBox" class="log-box">
        <div v-for="(l, i) in logs" :key="i" class="log-line" :class="l.level">{{ l.text }}</div>
      </div>
      <div v-if="cancelled && phase === 'running'" class="cancel-hint">
        {{ t("Cancelling…") }}
      </div>
    </div>

    <template #footer>
      <template v-if="phase === 'running'">
        <button class="btn" @click="cancel">{{ t("Cancel") }}</button>
      </template>
      <template v-else-if="phase === 'done'">
        <button class="btn primary" @click="close">{{ t("Close") }}</button>
      </template>
      <template v-else>
        <button class="btn" @click="close">{{ t("Cancel") }}</button>
        <button class="btn primary" :disabled="!canClone" @click="clone()">
          {{ t("Clone Repository") }}
        </button>
      </template>
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

.check-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  cursor: pointer;
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

.dest-preview {
  color: var(--text-tertiary);
  font-size: 12px;
  margin-top: -6px;
}

.progress-area {
  padding: 20px 4px 6px;
}

.progress-title {
  font-size: 13px;
  margin-bottom: 10px;
  color: var(--text-secondary);
}

.bar {
  height: 6px;
  background: var(--bg-active);
  border-radius: 3px;
  overflow: hidden;
}

.bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width 0.2s;
}

.bar-fill.done {
  background: var(--status-added, #2da44e);
}

.cancel-hint {
  margin-top: 8px;
  color: var(--status-modified);
  font-size: 12px;
}

.log-box {
  height: 260px;
  overflow-y: auto;
  background: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  font-family: ui-monospace, Consolas, monospace;
  font-size: 12px;
  line-height: 1.55;
  /* 覆盖全局 user-select:none：日志文本可选取复制 */
  user-select: text;
  cursor: text;
}

.log-line {
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text-secondary);
}

.log-line.dim {
  color: var(--text-tertiary);
}

.log-line.remote {
  color: var(--accent);
}

.log-line.ok {
  color: var(--status-added, #2da44e);
  font-weight: 600;
}

.log-line.warn {
  color: var(--status-modified);
}

.log-line.error {
  color: var(--status-deleted);
  font-weight: 600;
}
</style>
