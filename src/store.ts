/** 全局应用状态与动作（Vue reactive，无额外状态库） */
import { reactive, watch } from "vue";
import { api, HigitApiError, onStatusUpdated } from "./api";
import type {
  CommandErrorDto,
  ConfigDto,
  FileRowDto,
  RepoSummaryDto,
  StatusDto,
  TreeRowDto,
} from "./api/types";
import { currentLocale, setLocale, t } from "./i18n";

export interface ConfirmState {
  visible: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  danger: boolean;
  resolve: ((ok: boolean) => void) | null;
}

export interface InputState {
  visible: boolean;
  title: string;
  label: string;
  value: string;
  resolve: ((v: string | null) => void) | null;
}

export interface CredentialsPayload {
  kind: "http" | "ssh";
  host: string;
  username: string;
  /** 凭证提交后要重试的同步任务；null = 克隆流程（由 onCreds 回调处理） */
  retry: { kind: "push" | "pull" | "fetch"; branch: string } | null;
  /** 凭证提交回调（克隆流程使用） */
  onCreds?: ((creds: { kind: "http" | "ssh"; username: string; password?: string; keyPath?: string; passphrase?: string }, remember: boolean) => void) | null;
}

export interface ErrorPayload {
  message: string;
  raw: string;
}

export type SyncKind = "push" | "pull" | "fetch";

export interface UiState {
  newRepo: boolean;
  clone: boolean;
  branches: boolean;
  settings: boolean;
  repoSettingsId: string | null;
  /** 同步对话框（推送 / 拉取 / 抓取）；null = 关闭 */
  syncDialog: SyncKind | null;
  /** 提交对话框（FR-4 扩展） */
  commitDialog: boolean;
  /** 撤销修改对话框（工作区右键）：target = 目录（isFile=false，空白区为当前浏览目录）或文件 */
  discard: { target: string; isFile: boolean } | null;
  /** 分支操作对话框（工作区右键）：switch = 切换分支；merge = 合并分支 */
  branchOp: "switch" | "merge" | null;
  legend: boolean;
  about: boolean;
  shortcuts: boolean;
  diff: { path: string; spec: "workdir" | "staged" } | null;
  commitDiff: { oid: string; path: string } | null;
  credentials: CredentialsPayload | null;
  identityGuide: boolean;
  error: ErrorPayload | null;
  progress: { title: string; stage: string; percent: number; jobId: number } | null;
}

export const store = reactive({
  ready: false,
  config: null as ConfigDto | null,
  version: "",
  systemTheme: "light" as "light" | "dark",
  /** 系统语言（后端 sys-locale 读取），language 为空（跟随系统）时使用 */
  systemLanguage: "",

  repos: [] as RepoSummaryDto[],
  repoOpen: false,
  repoPath: "",
  repoName: "",
  status: null as StatusDto | null,
  /** 状态栏"当前用户"：生效 Git 身份文本 */
  identityText: "",

  currentDir: "",
  fileRows: [] as FileRowDto[],
  treeExpanded: new Set<string>(),
  treeRows: [] as TreeRowDto[],

  tab: "changes" as "changes" | "history",
  sidebarVisible: true,

  /** 当前长任务（状态栏 + 进度对话框） */
  task: null as null | { jobId: number; kind: string; stage: string; done: number; total: number },

  toasts: [] as { id: number; text: string }[],
  confirm: {
    visible: false,
    title: "",
    message: "",
    confirmLabel: "",
    danger: false,
    resolve: null,
  } as ConfirmState,
  input: { visible: false, title: "", label: "", value: "", resolve: null } as InputState,
  ui: {
    newRepo: false,
    clone: false,
    branches: false,
    settings: false,
    repoSettingsId: null,
    syncDialog: null,
    commitDialog: false,
    discard: null,
    branchOp: null,
    legend: false,
    about: false,
    shortcuts: false,
    diff: null,
    commitDiff: null,
    credentials: null,
    identityGuide: false,
    error: null,
    progress: null,
  } as UiState,
});

let toastSeq = 0;

export function toast(text: string) {
  const id = ++toastSeq;
  store.toasts.push({ id, text });
  setTimeout(() => {
    const i = store.toasts.findIndex((x) => x.id === id);
    if (i >= 0) store.toasts.splice(i, 1);
  }, 4000);
}

export function showConfirm(opts: {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
}): Promise<boolean> {
  return new Promise((resolve) => {
    Object.assign(store.confirm, {
      visible: true,
      title: opts.title,
      message: opts.message,
      confirmLabel: opts.confirmLabel ?? t("Confirm"),
      danger: opts.danger ?? false,
      resolve,
    });
  });
}

export function showInput(opts: { title: string; label: string; value?: string }): Promise<string | null> {
  return new Promise((resolve) => {
    Object.assign(store.input, {
      visible: true,
      title: opts.title,
      label: opts.label,
      value: opts.value ?? "",
      resolve,
    });
  });
}

/** 错误码 → 本地化文案键 */
export function errorCodeKey(code: string): string {
  const map: Record<string, string> = {
    "invalid-repository": "This is not a valid Git repository",
    "status-failed": "Failed to read repository status",
    "identity-missing": "Git identity is not configured (user.name / user.email)",
    "auth-failed": "Authentication failed",
    "network-failed": "Network operation failed",
    "merge-conflict": "Merge conflict",
    "no-upstream": "No upstream configured for this branch",
    diverged: "Branches have diverged; fast-forward is not possible",
    "push-rejected": "Push rejected — the remote contains commits you don't have. Pull first.",
    "dirty-worktree": "Working tree has uncommitted changes that conflict with this operation",
    "branch-not-merged": "This branch has commits that are not merged yet",
    cancelled: "Operation cancelled",
    "not-implemented": "This feature is not implemented yet",
    "no-repo": "No repository open",
  };
  return map[code] ?? "Operation failed";
}

export function showError(err: CommandErrorDto | HigitApiError | Error) {
  if (err instanceof HigitApiError) {
    store.ui.error = {
      message: t(errorCodeKey(err.code), Object.fromEntries(err.params.map((p, i) => [String(i), p]))),
      raw: err.raw ?? "",
    };
    return;
  }
  if ("code" in err) {
    const e = err as CommandErrorDto;
    store.ui.error = { message: t(errorCodeKey(e.code)), raw: e.raw ?? "" };
    return;
  }
  store.ui.error = { message: (err as Error).message, raw: "" };
}

// ---------- 主题 / 语言 ----------

export function effectiveTheme(): "light" | "dark" {
  const theme = store.config?.theme ?? "light";
  if (theme === "light" || theme === "dark") return theme;
  return store.systemTheme;
}

export function applyTheme() {
  document.documentElement.dataset.theme = effectiveTheme();
}

export function applyLanguage() {
  const lang = store.config?.language ?? "";
  if (lang === "en" || lang === "zh-CN") {
    setLocale(lang);
    return;
  }
  // 跟随系统：优先后端读取的系统 locale，回退 WebView navigator.language
  const sys = store.systemLanguage || navigator.language;
  setLocale(String(sys).toLowerCase().startsWith("zh") ? "zh-CN" : "en");
}

export async function persistConfig(patch?: Partial<ConfigDto>) {
  if (!store.config) return;
  if (patch) Object.assign(store.config, patch);
  try {
    await api.saveConfig(JSON.parse(JSON.stringify(store.config)));
  } catch (e) {
    showError(e as Error);
  }
}

// ---------- 数据加载 ----------

export async function refreshRepos() {
  try {
    store.repos = await api.listRepos();
  } catch (e) {
    showError(e as Error);
  }
}

/** 状态栏当前用户（生效身份：仓库设置 → git 配置 → 全局） */
export async function refreshIdentity() {
  try {
    const id = await api.getEffectiveIdentity();
    store.identityText = id ? `${id.name} <${id.email}>` : "";
  } catch {
    store.identityText = "";
  }
}

export async function refreshStatus(): Promise<StatusDto | null> {
  if (!store.repoOpen) return null;
  try {
    store.status = await api.refreshStatus();
    await refreshFiles();
    return store.status;
  } catch (e) {
    showError(e as Error);
    return null;
  }
}

export async function refreshFiles() {
  if (!store.repoOpen) return;
  try {
    store.fileRows = await api.listDir(store.currentDir, store.config?.showIgnored ?? false);
  } catch (e) {
    showError(e as Error);
  }
}

export async function refreshTree() {
  if (!store.repoOpen) return;
  try {
    store.treeRows = await api.listTree([...store.treeExpanded]);
  } catch (e) {
    showError(e as Error);
  }
}

export async function navigate(rel: string) {
  store.currentDir = rel;
  await refreshFiles();
}

export async function openRepo(path: string) {
  try {
    const view = await api.openRepo(path);
    store.repoOpen = true;
    store.repoPath = view.path;
    store.repoName = view.name;
    store.status = view.status;
    store.currentDir = "";
    store.treeExpanded = new Set([view.path]);
    store.tab = "changes";
    await Promise.all([refreshFiles(), refreshTree(), refreshRepos(), refreshIdentity()]);
  } catch (e) {
    showError(e as Error);
  }
}

export async function closeRepo() {
  try {
    await api.closeRepo();
  } catch {
    /* 忽略 */
  }
  store.repoOpen = false;
  store.repoPath = "";
  store.repoName = "";
  store.status = null;
  store.fileRows = [];
  store.treeRows = [];
  store.currentDir = "";
}

export function removeRepoEntry(path: string) {
  store.repos = store.repos.filter((r) => r.path !== path);
}

// ---------- 状态推送（status → 行 → store） ----------

function applyStatusDto(dto: StatusDto) {
  store.status = dto;
  void refreshFiles();
  void refreshTree();
}

export async function bootstrap() {
  try {
    const boot = await api.getState();
    store.config = boot.config;
    store.version = boot.version;
    store.systemTheme = boot.systemTheme;
    store.systemLanguage = boot.systemLanguage;
    applyLanguage();
    applyTheme();
    if (boot.configCorrupted) {
      store.ui.error = {
        message: t("Configuration file is corrupted; default settings are used"),
        raw: "",
      };
    }
    store.ready = true;

    onStatusUpdated((e) => {
      if (e.path === store.repoPath) {
        applyStatusDto(e.status);
        // 仓库卡片变更数徽标联动
        void refreshRepos();
      }
    });

    await refreshRepos();

    // 系统主题变化监听（跟随系统模式）
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", (ev) => {
      store.systemTheme = ev.matches ? "dark" : "light";
      applyTheme();
    });

    // 恢复上次打开的仓库（FR-1.7）
    if (boot.config.lastRepo) {
      await openRepo(boot.config.lastRepo);
    }

    // Git 身份缺失首次引导（FR-4.2）
    const identity = await api.getEffectiveIdentity().catch(() => null);
    if (!identity) {
      store.ui.identityGuide = true;
    }
  } catch (e) {
    showError(e as Error);
  }
}

function applyConfigSideEffects() {
  applyLanguage();
  applyTheme();
}

export function watchConfig() {
  watch(
    () => [store.config?.language, store.config?.theme],
    () => applyConfigSideEffects(),
  );
  watch(
    () => store.config?.showIgnored,
    () => void refreshFiles(),
  );
  watch(
    () => store.config?.autoRefresh,
    () => void refreshStatus(),
  );
}

export function localeNow() {
  return currentLocale();
}
