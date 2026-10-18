<script setup lang="ts">
/** 同步对话框（推送 / 拉取 / 抓取，FR-5.2 扩展）：版本信息 / 认证 / Git 身份 三页；
 * 推送=多分支+目标分支+Force，拉取=单选分支（默认当前），抓取=全部分支只读展示；
 * 开始后切换到进度 + 详细日志页（目标地址 / 认证方式 / 待传输内容 / 远端输出 / 结果） */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { store, type SyncKind, showError } from "../store";
import { api, onJobProgress, onSyncLog, replaySyncLogs } from "../api";
import type { JobDoneEvent, SyncLogEvent, SyncPreviewDto, RepoEntryDto } from "../api/types";
import { t } from "../i18n";
import { isValidEmail } from "../validate";
import { registerSyncDone, startSyncJob, type SyncCreds, type SyncJobReq } from "../tasks";
import DialogBase from "./DialogBase.vue";
import PasswordInput from "./PasswordInput.vue";

const emit = defineEmits<{ close: [] }>();

const tab = ref<0 | 1 | 2>(0);
const preview = ref<SyncPreviewDto | null>(null);
const entry = ref<RepoEntryDto | null>(null);
const repoId = ref("");
const loadError = ref("");

// 阶段：setup 配置 → running 执行中 → done 结束（日志保留可关闭）
const phase = ref<"setup" | "running" | "done">("setup");
const jobId = ref(0);
const logs = ref<{ level: string; text: string }[]>([]);
const percent = ref(0);
const stage = ref("");
const logBox = ref<HTMLElement | null>(null);

const mode = computed<SyncKind>(() => store.ui.syncDialog ?? "push");

// 版本信息页：分支选择（push 多选 / pull 单选 / fetch 只读）+ 目标远端分支 + Force
const selected = ref<string[]>([]);
const targets = ref<Record<string, string>>({});
const force = ref(false);

// 拉取选项（对应 TortoiseGit Pull）：整合方式 + 合并选项 + prune / tags
const pullRebase = ref(false);
const pullNoFF = ref(false);
const pullFFOnly = ref(false);
const pullNoCommit = ref(false);
const pullSquash = ref(false);
const pullAllTags = ref(false);
const pullPrune = ref(true);

// --no-ff 与 --ff-only 互斥；--squash 隐含 --no-commit
watch(pullNoFF, (v) => {
  if (v) pullFFOnly.value = false;
});
watch(pullFFOnly, (v) => {
  if (v) pullNoFF.value = false;
});

// 认证页 / Git 身份页（与克隆对话框一致；有改动才回写仓库设置）
const authKind = ref<"http" | "ssh">("http");
const username = ref("");
const password = ref("");
const keyPath = ref("");
const rememberCred = ref(true);
const authTouched = ref(false);
const identityName = ref("");
const identityEmail = ref("");
const identityTouched = ref(false);
const globalId = ref<{ name: string; email: string } | null>(null);

/** 邮箱非空但格式非法：禁止执行同步操作并提示 */
const identityEmailInvalid = computed(
  () => identityEmail.value.trim() !== "" && !isValidEmail(identityEmail.value),
);

/** 认证方式单选悬停提示：该认证方式对应的仓库地址格式（computed 使文案跟随语言切换刷新） */
const httpsUrlTip = computed(() =>
  t("Corresponding repository URL format: {0}", { 0: "https://github.com/xxxxxx/yyyyyy.git" }),
);
const sshUrlTip = computed(() =>
  t("Corresponding repository URL format: {0}", { 0: "git@github.com:xxxxxx/yyyyyy.git" }),
);

/** 输入留空的字段回退全局配置（仓库条目已预填的值优先）；解析出完整姓名邮箱才允许执行 */
const resolvedName = computed(() => identityName.value.trim() || globalId.value?.name || "");
const resolvedEmail = computed(() => identityEmail.value.trim() || globalId.value?.email || "");
const identityComplete = computed(() => !!resolvedName.value && !!resolvedEmail.value);

/** 系统凭证管理器已存有该仓库的 HTTPS 凭证：密码 / Token 可留空沿用 */
const credSaved = ref(false);
/** 认证必填：HTTPS 需用户名 + 密码 / Token（已存凭证可沿用）；SSH 需密钥文件 */
const authComplete = computed(() =>
  authKind.value === "ssh"
    ? !!keyPath.value.trim()
    : (!!username.value.trim() && !!password.value) || credSaved.value,
);

let lastPayload: Omit<SyncJobReq, "creds" | "remember"> | null = null;

const tabs = computed(() => [t("Version Info"), t("Auth"), t("Git Identity")]);
const isSshUrl = computed(() => {
  const u = preview.value?.url ?? "";
  return u.startsWith("git@") || u.startsWith("ssh://");
});
const noRemote = computed(() => !!preview.value && !preview.value.remoteName && !preview.value.url);
const canStart = computed(
  () =>
    phase.value === "setup" &&
    !noRemote.value &&
    (mode.value === "fetch" || selected.value.length > 0) &&
    identityComplete.value &&
    !identityEmailInvalid.value &&
    authComplete.value,
);
const allSelected = computed(
  () =>
    !!preview.value &&
    preview.value.branches.length > 0 &&
    selected.value.length === preview.value.branches.length,
);

const kindTitle = computed(() =>
  mode.value === "push" ? t("Push") : mode.value === "pull" ? t("Pull") : t("Fetch"),
);
const urlLabel = computed(() =>
  mode.value === "push"
    ? t("Push target remote")
    : mode.value === "pull"
      ? t("Pull target remote")
      : t("Fetch target remote"),
);
const listLabel = computed(() =>
  mode.value === "push" ? t("Select branch to push") : t("Select branch to pull"),
);

const dlgTitle = computed(() =>
  phase.value === "setup"
    ? kindTitle.value
    : `${preview.value?.name || store.repoName} ${kindTitle.value} ${t("Progress")}`,
);

/** 抓取对话框展示的只读分支行：本地 + 远端（origin/xxx） */
const fetchRows = computed(() => {
  const p = preview.value;
  if (!p) return [] as { name: string; summary: string }[];
  const local = p.branches.map((b) => ({ name: b.name, summary: b.summary }));
  const remote = p.remoteBranches.map((r) => ({
    name: p.remoteName ? `${p.remoteName}/${r}` : r,
    summary: "",
  }));
  return [...local, ...remote];
});

function toggleBranch(name: string) {
  if (mode.value === "push") {
    const i = selected.value.indexOf(name);
    if (i >= 0) selected.value.splice(i, 1);
    else selected.value.push(name);
  } else {
    // 拉取：单选
    selected.value = [name];
  }
}

function toggleAll() {
  if (!preview.value || mode.value !== "push") return;
  selected.value = allSelected.value ? [] : preview.value.branches.map((b) => b.name);
}

function markAuth() {
  authTouched.value = true;
}

function markIdentity() {
  identityTouched.value = true;
}

async function chooseKey() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({ title: t("SSH key file") });
  if (file && typeof file === "string") {
    keyPath.value = file;
    markAuth();
  }
}

onMounted(async () => {
  repoId.value = store.repos.find((r) => r.path === store.repoPath)?.id ?? "";
  try {
    preview.value = await api.syncPreview();
    // 默认选中当前分支（push 可多选 / pull 单选；fetch 无需选择）
    if (mode.value !== "fetch") {
      const cur = preview.value.branches.find((b) => b.isCurrent) ?? preview.value.branches[0];
      if (cur) selected.value = [cur.name];
    }
    const targetsMap: Record<string, string> = {};
    for (const b of preview.value.branches) targetsMap[b.name] = b.name;
    targets.value = targetsMap;
  } catch (e) {
    loadError.value = (e as Error).message ?? String(e);
  }
  // 认证 / Git 身份：仓库设置优先，其次全局身份；SSH/HTTPS 按 URL 推断
  try {
    if (repoId.value) entry.value = await api.getRepoSettings(repoId.value);
  } catch {
    /* 不在列表中：按默认处理 */
  }
  authKind.value = entry.value?.auth
    ? entry.value.auth.kind === "ssh"
      ? "ssh"
      : "http"
    : isSshUrl.value
      ? "ssh"
      : "http";
  username.value = entry.value?.auth?.username ?? "";
  keyPath.value = entry.value?.auth?.keyPath ?? "";
  if (entry.value?.identity) {
    identityName.value = entry.value.identity.name;
    identityEmail.value = entry.value.identity.email;
  } else {
    // 不预填全局值：输入留空时回退全局配置，输入框下方提示将采用的全局值
    try {
      globalId.value = await api.getGlobalIdentity();
    } catch {
      /* ignore */
    }
  }
  // 已存 HTTPS 凭证（密码不回显，留空沿用）：决定认证字段是否必填
  credSaved.value = repoId.value
    ? await api.repoCredentialSaved(repoId.value).catch(() => false)
    : false;

  unlistenLog = await onSyncLog(handleLog);
  unlistenProgress = await onJobProgress((e) => {
    if (e.jobId !== jobId.value) return;
    // 无对象可写时 git2 上报 0/0：不显示计数，避免"推送对象 0/0"的困惑
    if (e.total > 0) {
      stage.value = `${t(e.stage)} ${e.done}/${e.total}`;
      percent.value = Math.floor((e.done * 100) / e.total);
    } else {
      stage.value = t(e.stage);
    }
  });
  registerSyncDone(handleDone);
  window.addEventListener("higit:sync-esc", onEsc);
});

let unlistenLog: (() => void) | null = null;
let unlistenProgress: (() => void) | null = null;

function onEsc() {
  close();
}

function handleLog(e: SyncLogEvent) {
  if (e.jobId !== jobId.value) return;
  logs.value.push({ level: e.level, text: e.text });
  void nextTick(() => {
    if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight;
  });
}

function handleDone(e: JobDoneEvent) {
  if (e.jobId !== jobId.value) return;
  phase.value = "done";
  if (e.ok) percent.value = 100;
  // 认证失败：拉起凭证对话框，提交后带新凭证重试同样的任务
  if (!e.ok && e.error?.code === "auth-failed" && e.error.auth) {
    const auth = e.error.auth;
    store.ui.credentials = {
      kind: auth.kind,
      host: auth.host,
      username: auth.username,
      retry: null,
      onCreds: (creds) => {
        store.ui.credentials = null;
        void retrySync(creds);
      },
    };
  }
}

async function retrySync(creds: SyncCreds) {
  if (!lastPayload) return;
  logs.value.push({ level: "info", text: t("Retrying with new credentials…") });
  phase.value = "running";
  percent.value = 0;
  stage.value = "";
  try {
    jobId.value = await startSyncJob({ ...lastPayload, creds, remember: false });
    replaySyncLogs(jobId.value); // 补发早于 jobId 注册到达的开头日志（认证方式/出网方式等）
  } catch (e) {
    phase.value = "done";
    showError(e as Error);
  }
}

function buildCreds(): SyncCreds | null {
  if (authKind.value === "ssh") {
    return { kind: "ssh", username: username.value.trim() || "git", keyPath: keyPath.value.trim() };
  }
  return username.value.trim() && password.value
    ? { kind: "http", username: username.value.trim(), password: password.value }
    : null;
}

async function doStart() {
  if (!canStart.value || !preview.value) return;

  // 有改动的认证 / Git 身份回写仓库设置（未动过的字段原样保留）
  if (authTouched.value || identityTouched.value) {
    try {
      await api.saveRepoSettings({
        id: repoId.value,
        path: store.repoPath,
        alias: entry.value?.alias ?? null,
        auth: authTouched.value
          ? authKind.value === "ssh"
            ? { kind: "ssh", username: username.value.trim() || "git", keyPath: keyPath.value.trim() }
            : username.value.trim()
              ? { kind: "http", username: username.value.trim(), keyPath: "" }
              : null
          : entry.value?.auth ?? null,
        url: entry.value?.url ?? preview.value.url,
        identity: identityTouched.value
          ? identityComplete.value
            ? { name: resolvedName.value, email: resolvedEmail.value }
            : null
          : entry.value?.identity ?? null,
      });
    } catch {
      /* 尽力而为 */
    }
  }
  // HTTPS 勾选"记住"且填了密码：写入系统凭证管理器
  const creds = buildCreds();
  if (rememberCred.value && creds?.kind === "http" && creds.password) {
    const host = hostFromUrl(preview.value.url);
    if (host) void api.saveCredential(host, creds).catch(() => undefined);
  }

  // 组装各模式的任务参数
  let payload: Omit<SyncJobReq, "creds" | "remember">;
  if (mode.value === "push") {
    payload = {
      kind: "push",
      refspecs: selected.value.map((local) => ({
        local,
        remote: targets.value[local]?.trim() || local,
      })),
      force: force.value,
    };
  } else if (mode.value === "pull") {
    payload = {
      kind: "pull",
      branch: selected.value[0],
      pullOpts: {
        rebase: pullRebase.value,
        noFf: pullNoFF.value,
        ffOnly: pullFFOnly.value,
        noCommit: !pullSquash.value && pullNoCommit.value,
        squash: pullSquash.value,
        allTags: pullAllTags.value,
        prune: pullPrune.value,
      },
    };
  } else {
    payload = { kind: "fetch" };
  }
  lastPayload = payload;

  phase.value = "running";
  logs.value = [];
  percent.value = 0;
  stage.value = kindTitle.value + "…";
  try {
    jobId.value = await startSyncJob({ ...payload, creds, remember: rememberCred.value });
    replaySyncLogs(jobId.value); // 补发早于 jobId 注册到达的开头日志（认证方式/出网方式等）
  } catch (e) {
    phase.value = "done";
    showError(e as Error);
  }
}

function hostFromUrl(url: string): string | null {
  if (url.startsWith("git@")) {
    const host = url.slice(4).split(":")[0] ?? "";
    return host || null;
  }
  const idx = url.indexOf("://");
  if (idx < 0) return null;
  const authority = url.slice(idx + 3).split("/")[0] ?? "";
  const noUser = authority.includes("@") ? authority.slice(authority.lastIndexOf("@") + 1) : authority;
  const host = noUser.split(":")[0] ?? "";
  return host || null;
}

async function cancelSync() {
  if (jobId.value) await api.cancelJob(jobId.value).catch(() => undefined);
}

function close() {
  if (phase.value === "running") void cancelSync();
  emit("close");
}

onUnmounted(() => {
  unlistenLog?.();
  unlistenProgress?.();
  registerSyncDone(null);
  window.removeEventListener("higit:sync-esc", onEsc);
});
</script>

<template>
  <DialogBase :title="dlgTitle" :width="640" :close-on-mask="false" @close="close">
    <template v-if="phase === 'setup'">
      <div class="repo-path">{{ preview?.path ?? store.repoPath }}</div>
      <div class="tabs">
        <button
          v-for="(name, i) in tabs"
          :key="name"
          class="tab-chip"
          :class="{ on: tab === i }"
          @click="tab = i as 0 | 1 | 2"
        >
          {{ name }}
        </button>
      </div>

      <div v-if="loadError" class="danger-hint">{{ loadError }}</div>

      <!-- 版本信息 -->
      <div v-show="tab === 0">
        <div class="form-row">
          <label>{{ urlLabel }}</label>
          <div class="url-line">{{ preview?.url || "—" }}</div>
        </div>
        <div v-if="noRemote" class="danger-hint">{{ t("No remote configured for this repository") }}</div>

        <!-- 推送：多选 + 目标远端分支 + Force -->
        <template v-if="mode === 'push'">
          <div class="list-head">
            <label>{{ listLabel }}</label>
            <label class="check-line all-line">
              <input type="checkbox" :checked="allSelected" @change="toggleAll" />
              <span>{{ t("All") }}</span>
            </label>
          </div>
          <div class="branch-list">
            <div
              v-for="b in preview?.branches ?? []"
              :key="b.name"
              class="branch-row"
              :class="{ on: selected.includes(b.name) }"
              @click="toggleBranch(b.name)"
            >
              <input
                type="checkbox"
                :checked="selected.includes(b.name)"
                @click.stop
                @change="toggleBranch(b.name)"
              />
              <div class="branch-main">
                <div class="branch-name">
                  <b>{{ b.name }}</b>
                  <span v-if="b.isCurrent" class="badge">{{ t("Current") }}</span>
                </div>
                <div class="branch-sub ellipsis" :title="b.summary">{{ b.summary }}</div>
              </div>
              <div class="branch-side" @click.stop>
                <span class="short-id">{{ b.shortId }}</span>
                <input
                  v-model="targets[b.name]"
                  class="target-input"
                  :title="t('Remote branch')"
                  :placeholder="b.name"
                />
              </div>
            </div>
            <div v-if="preview && preview.branches.length === 0" class="empty-hint">
              {{ t("No local branches") }}
            </div>
          </div>

          <label class="check-line force-line">
            <input v-model="force" type="checkbox" />
            <span>{{ t("Force push (overwrite remote branch)") }}</span>
          </label>
          <div v-if="force" class="danger-hint">
            {{ t("Force push overwrites the remote branch history. Replaced commits may be lost permanently and other collaborators will be affected. Make sure you understand the consequences.") }}
          </div>
        </template>

        <!-- 拉取：单选分支（默认当前分支） -->
        <template v-else-if="mode === 'pull'">
          <div class="list-head">
            <label>{{ listLabel }}</label>
          </div>
          <div class="branch-list">
            <div
              v-for="b in preview?.branches ?? []"
              :key="b.name"
              class="branch-row"
              :class="{ on: selected.includes(b.name) }"
              @click="toggleBranch(b.name)"
            >
              <input
                type="checkbox"
                :checked="selected.includes(b.name)"
                @click.stop
                @change="toggleBranch(b.name)"
              />
              <div class="branch-main">
                <div class="branch-name">
                  <b>{{ b.name }}</b>
                  <span v-if="b.isCurrent" class="badge">{{ t("Current") }}</span>
                </div>
                <div class="branch-sub ellipsis" :title="b.summary">{{ b.summary }}</div>
              </div>
              <div class="branch-side" @click.stop>
                <span v-if="b.behind > 0" class="behind">↓{{ b.behind }}</span>
                <span class="short-id">{{ b.shortId }}</span>
              </div>
            </div>
            <div v-if="preview && preview.branches.length === 0" class="empty-hint">
              {{ t("No local branches") }}
            </div>
          </div>

          <!-- 拉取选项（对应 TortoiseGit Pull 的 Option 区） -->
          <div class="pull-opts">
            <div class="po-row">
              <span class="po-label">{{ t("Integration") }}:</span>
              <label class="check-line">
                <input v-model="pullRebase" type="radio" :value="false" />
                <span>{{ t("Merge") }}</span>
              </label>
              <label
                class="check-line"
                :title="t('Replay local commits on top of the fetched tip (git pull --rebase); on conflict the rebase is aborted and the original branch restored')"
              >
                <input v-model="pullRebase" type="radio" :value="true" />
                <span>{{ t("Rebase") }}</span>
              </label>
            </div>
            <div v-if="!pullRebase" class="po-row">
              <label
                class="check-line"
                :title="t('Always create a merge commit even when fast-forward is possible (git merge --no-ff)')"
              >
                <input v-model="pullNoFF" type="checkbox" />
                <span>{{ t("No Fast Forward") }}</span>
              </label>
              <label
                class="check-line"
                :title="t('Abort unless the merge can be resolved as a fast-forward (git merge --ff-only)')"
              >
                <input v-model="pullFFOnly" type="checkbox" />
                <span>{{ t("Fast Forward Only") }}</span>
              </label>
              <label
                class="check-line"
                :class="{ off: pullSquash }"
                :title="t('Perform the merge but stop before creating the merge commit (git merge --no-commit)')"
              >
                <input v-model="pullNoCommit" type="checkbox" :disabled="pullSquash" />
                <span>{{ t("No Commit") }}</span>
              </label>
              <label
                class="check-line"
                :title="t('Stage the merge result without creating a merge commit (git merge --squash)')"
              >
                <input v-model="pullSquash" type="checkbox" />
                <span>{{ t("Squash") }}</span>
              </label>
            </div>
            <div class="po-row">
              <label
                class="check-line"
                :title="t('Remove remote-tracking branches deleted on the remote (git fetch --prune)')"
              >
                <input v-model="pullPrune" type="checkbox" />
                <span>{{ t("Prune") }}</span>
              </label>
              <label
                class="check-line"
                :title="t('Fetch all tags instead of only reachable ones (git fetch --tags)')"
              >
                <input v-model="pullAllTags" type="checkbox" />
                <span>{{ t("Tags") }}</span>
              </label>
            </div>
          </div>
        </template>

        <!-- 抓取：所有分支只读展示（抓取作用于远端全部分支） -->
        <template v-else>
          <div class="list-head">
            <label>{{ t("Fetches all branches from the remote") }}</label>
          </div>
          <div class="branch-list readonly">
            <div v-for="r in fetchRows" :key="r.name" class="branch-row flat">
              <div class="branch-main">
                <div class="branch-name">
                  <span>{{ r.name }}</span>
                </div>
                <div class="branch-sub ellipsis" :title="r.summary">{{ r.summary }}</div>
              </div>
            </div>
            <div v-if="fetchRows.length === 0" class="empty-hint">{{ t("No local branches") }}</div>
          </div>
        </template>
      </div>

      <!-- 认证 -->
      <div v-show="tab === 1">
        <div class="form-row">
          <div class="kind-row">
            <label class="check-line" :title="httpsUrlTip"><input v-model="authKind" type="radio" value="http" @change="markAuth" /> HTTPS</label>
            <label class="check-line" :title="sshUrlTip"><input v-model="authKind" type="radio" value="ssh" @change="markAuth" /> SSH</label>
          </div>
        </div>
        <div class="hint-line">{{ t("Applies to this clone and later pull / push / fetch of this repository") }}</div>
        <template v-if="authKind === 'http'">
          <div class="form-row">
            <label>{{ t("Username") }}</label>
            <input v-model="username" placeholder="username" @input="markAuth" />
          </div>
          <div class="form-row">
            <label>{{ t("Password / Token") }}</label>
            <PasswordInput v-model="password" @input="markAuth" />
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
              <input v-model="keyPath" @input="markAuth" />
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

      <!-- Git 身份 -->
      <div v-show="tab === 2">
        <div class="identity-grid">
          <div class="form-row">
            <label>{{ t("Your Name") }}</label>
            <input v-model="identityName" @input="markIdentity" />
            <span v-if="!identityName.trim() && globalId?.name" class="fallback-hint">
              {{ t("Leave empty to use the global identity: {0}", { 0: globalId.name }) }}
            </span>
          </div>
          <div class="form-row">
            <label>{{ t("Email") }}</label>
            <input v-model="identityEmail" @input="markIdentity" />
            <span v-if="identityEmailInvalid" class="invalid-hint">
              {{ t("Invalid email format") }}
            </span>
            <span v-else-if="!identityEmail.trim() && globalId?.email" class="fallback-hint">
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
    </template>

    <!-- 执行中 / 结束：进度条 + 详细日志（TortoiseGit 风格） -->
    <div v-else class="progress-area">
      <div class="progress-title">{{ stage || kindTitle + "…" }}</div>
      <div class="bar">
        <div class="bar-fill" :class="{ done: phase === 'done' }" :style="{ width: percent + '%' }" />
      </div>
      <div ref="logBox" class="log-box">
        <div v-for="(l, i) in logs" :key="i" class="log-line" :class="l.level">{{ l.text }}</div>
      </div>
    </div>

    <template #footer>
      <template v-if="phase === 'setup'">
        <button class="btn" @click="close">{{ t("Cancel") }}</button>
        <button class="btn primary" :disabled="!canStart" @click="doStart">{{ kindTitle }}</button>
      </template>
      <template v-else-if="phase === 'running'">
        <button class="btn" @click="cancelSync">{{ t("Abort") }}</button>
      </template>
      <template v-else>
        <button class="btn primary" @click="close">{{ t("Close") }}</button>
      </template>
    </template>
  </DialogBase>
</template>

<style scoped>
.repo-path {
  color: var(--text-tertiary);
  font-size: 12px;
  margin-bottom: 12px;
}

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

.form-inline {
  display: flex;
  gap: 8px;
}

.form-inline input {
  flex: 1;
}

.url-line {
  font-family: ui-monospace, Consolas, monospace;
  font-size: 13px;
  color: var(--text-primary);
  background: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  padding: 7px 10px;
  word-break: break-all;
}

.list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}

.list-head label:first-child {
  font-size: 12px;
  color: var(--text-secondary);
}

.all-line {
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
}

.branch-list {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  overflow: hidden;
  margin-bottom: 14px;
  max-height: 240px;
  overflow-y: auto;
}

.branch-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--divider);
  cursor: pointer;
}

.branch-row:last-child {
  border-bottom: none;
}

.branch-row.on {
  background: var(--accent-subtle);
  box-shadow: inset 0 0 0 1px var(--accent);
}

.branch-row.flat {
  cursor: default;
}

.branch-main {
  flex: 1;
  min-width: 0;
}

.branch-name {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}

.badge {
  font-size: 11px;
  color: var(--accent);
  border: 1px solid var(--accent);
  border-radius: 999px;
  padding: 0 8px;
  line-height: 18px;
}

.behind {
  font-size: 12px;
  color: var(--status-modified);
  font-weight: 600;
}

.branch-sub {
  color: var(--text-tertiary);
  font-size: 12px;
  margin-top: 2px;
}

.branch-side {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
  flex: none;
}

.short-id {
  font-family: ui-monospace, Consolas, monospace;
  font-size: 12px;
  color: var(--text-tertiary);
}

.target-input {
  width: 150px;
  font-size: 12px;
  padding: 3px 8px;
}

.check-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  cursor: pointer;
}

/* 拉取选项区（TortoiseGit Pull 的 Option 区） */
.pull-opts {
  margin-top: 10px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.po-row {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
}

.po-label {
  font-size: 12px;
  color: var(--text-tertiary);
}

.po-row .check-line.off {
  color: var(--text-disabled);
}

.force-line {
  margin-top: 4px;
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

.danger-hint {
  color: var(--status-deleted);
  font-size: 12px;
  background: var(--bg-card);
  border: 1px solid var(--status-deleted);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  margin-bottom: 12px;
  line-height: 1.5;
}

.empty-hint {
  padding: 14px;
  color: var(--text-tertiary);
  font-size: 12px;
}

.progress-area {
  padding: 8px 4px 6px;
}

.progress-title {
  font-size: 13px;
  margin-bottom: 10px;
  color: var(--text-secondary);
}

.bar {
  height: 8px;
  background: var(--bg-active);
  border-radius: 4px;
  overflow: hidden;
  margin-bottom: 12px;
}

.bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 4px;
  transition: width 0.2s;
}

.bar-fill.done {
  background: var(--status-added, #2da44e);
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
