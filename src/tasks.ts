/** 共享动作层：长任务（同步/克隆）事件处理 + 暂存 / 丢弃 / 提交 / 同步动作 */
import { api, HigitApiError, onJobDone, onJobProgress } from "./api";
import type { JobDoneEvent, StatusDto } from "./api/types";
import {
  store,
  showError,
  toast,
  refreshStatus,
  refreshFiles,
  refreshRepos,
  refreshIdentity,
  showConfirm,
  errorCodeKey,
  type SyncKind,
} from "./store";
import { t } from "./i18n";

let inited = false;

/** App 挂载时调用一次：注册长任务事件 */
export function initTasks() {
  if (inited) return;
  inited = true;
  onJobProgress((e) => {
    if (store.task?.jobId === e.jobId) {
      store.task.stage = e.stage;
      store.task.done = e.done;
      store.task.total = e.total;
      if (store.ui.progress?.jobId === e.jobId) {
        store.ui.progress.percent = e.total > 0 ? Math.floor((e.done * 100) / e.total) : 0;
        store.ui.progress.stage = t(e.stage) + (e.total > 0 ? ` ${e.done}/${e.total}` : "");
      }
    }
  });
  onJobDone(handleJobDone);
}

function handleJobDone(e: JobDoneEvent) {
  // 任务状态复位
  if (store.task?.jobId === e.jobId) store.task = null;
  if (store.ui.progress?.jobId === e.jobId) store.ui.progress = null;

  switch (e.kind) {
    case "clone":
      handleCloneDone(e);
      break;
    case "push":
    case "pull":
    case "fetch":
      handleSyncDone(e);
      break;
  }
}

function handleCloneDone(e: JobDoneEvent) {
  // 克隆对话框发起的任务：日志 / 结果由对话框展示，这里不再弹全局错误框
  if (cloneDoneHandler && e.jobId === cloneDoneJobId) {
    cloneDoneHandler(e);
    return;
  }
  const onCreds = cloneCredsCallback;
  if (e.ok) {
    finalizeCloneSuccess(e.repoPath);
  } else if (e.error) {
    if (e.error.code === "auth-failed" && e.error.auth) {
      // 凭证对话框（FR-1.2 / FR-5.5）；提交后回调重新发起克隆
      store.ui.credentials = {
        kind: e.error.auth.kind,
        host: e.error.auth.host,
        username: e.error.auth.username,
        retry: null,
        onCreds: (creds, remember) => onCreds?.(creds, remember),
      };
    } else if (e.error.code !== "cancelled") {
      showError(new HigitApiError(e.error));
      store.ui.clone = false;
    } else {
      toast(t("Operation cancelled"));
    }
  }
}

/** 克隆成功收尾：关对话框、登记进仓库列表并打开（对话框 / 兜底路径共用） */
export function finalizeCloneSuccess(repoPath?: string | null) {
  store.ui.clone = false;
  const path = repoPath;
  if (!path) return;
  void (async () => {
    const { openRepo } = await import("./store");
    try {
      await api.addLocalRepo(path); // 登记进仓库列表（openRepo 不会写入 config）
    } catch {
      /* 已登记或校验失败不阻断打开 */
    }
    await openRepo(path);
    toast(t("Clone Repository") + " ✓");
    cloneSuccessCallback?.(path);
  })();
}

let cloneDoneJobId = 0;
let cloneDoneHandler: ((e: JobDoneEvent) => void) | null = null;

/** 克隆对话框注册 job-done 处理（进度 / 日志 / 结果在对话框内展示）；传 null 注销 */
export function registerCloneDone(cb: ((e: JobDoneEvent) => void) | null) {
  cloneDoneHandler = cb;
}

/** 克隆对话框发起克隆后绑定 job_id（job-done 分流到对话框） */
export function bindCloneDialogJob(jobId: number) {
  cloneDoneJobId = jobId;
}

let cloneCredsCallback: ((creds: { kind: "http" | "ssh"; username: string; password?: string; keyPath?: string; passphrase?: string }, remember: boolean) => void) | null = null;

/** 克隆对话框注册凭证重试回调 */
export function registerCloneCreds(cb: typeof cloneCredsCallback) {
  cloneCredsCallback = cb;
}

let cloneSuccessCallback: ((repoPath: string) => void) | null = null;

/** 克隆对话框注册成功回调（入库认证 / 身份等非机密字段） */
export function registerCloneSuccess(cb: (repoPath: string) => void) {
  cloneSuccessCallback = cb;
}

function handleSyncDone(e: JobDoneEvent) {
  // 同步对话框发起的任务：日志/结果由对话框展示，这里只刷新，不弹 toast / 错误框
  if (e.kind !== "clone" && dialogJobId === e.jobId) {
    dialogDoneHandler?.(e);
  } else if (e.ok) {
    switch (e.kind) {
      case "push":
        toast(e.outcome?.createdUpstream ? t("Pushed and set upstream") : t("Pushed"));
        break;
      case "pull":
        toast(
          e.outcome?.updated
            ? `${t("Updated to")} ${e.outcome.headShortId ?? ""}`.trim()
            : t("Up to date"),
        );
        break;
      case "fetch":
        toast(t("Fetched"));
        break;
    }
  } else if (e.error) {
    if (e.error.code === "auth-failed" && e.error.auth) {
      // 凭证对话框：提交后重试原任务（FR-5.5 / FR-5.10）
      const pendingBranch = pendingSyncBranch;
      store.ui.credentials = {
        kind: e.error.auth.kind,
        host: e.error.auth.host,
        username: e.error.auth.username,
        retry: { kind: e.kind as "push" | "pull" | "fetch", branch: pendingBranch },
        onCreds: null,
      };
    } else if (e.error.code !== "cancelled") {
      showError(new HigitApiError(e.error));
    } else {
      toast(t("Operation cancelled"));
    }
  }
  void refreshStatus();
  void refreshRepos();
}

let pendingSyncBranch = "";

/** 发起推送 / 拉取 / 抓取（FR-5.10：进度对话框 + 取消） */
export async function startSync(kind: "push" | "pull" | "fetch", branch: string) {
  if (!store.repoOpen) {
    toast(t("No repository open"));
    return;
  }
  if (store.task) return; // 上一任务进行中
  pendingSyncBranch = branch;
  const titles: Record<string, string> = {
    push: "Pushing…",
    pull: "Fetching…",
    fetch: "Fetching…",
  };
  store.task = { jobId: 0, kind, stage: titles[kind] ?? "", done: 0, total: 0 };
  store.ui.progress = {
    title: `${t(kind === "push" ? "Push" : kind === "pull" ? "Pull" : "Fetch")}…`,
    stage: "",
    percent: 0,
    jobId: 0,
  };
  try {
    const jobId = await api.sync({ kind, branch, creds: null, remember: false });
    store.task!.jobId = jobId;
    store.ui.progress!.jobId = jobId;
  } catch (err) {
    store.task = null;
    store.ui.progress = null;
    showError(err as Error);
  }
}

/** 凭证对话框提交后重试同步任务 */
export async function retrySyncWithCreds(
  kind: "push" | "pull" | "fetch",
  branch: string,
  creds: { kind: "http" | "ssh"; username: string; password?: string; keyPath?: string; passphrase?: string },
) {
  if (store.task) return;
  pendingSyncBranch = branch;
  store.task = { jobId: 0, kind, stage: "", done: 0, total: 0 };
  store.ui.progress = {
    title: `${t(kind === "push" ? "Push" : kind === "pull" ? "Pull" : "Fetch")}…`,
    stage: "",
    percent: 0,
    jobId: 0,
  };
  try {
    const jobId = await api.sync({ kind, branch, creds, remember: false });
    store.task!.jobId = jobId;
    store.ui.progress!.jobId = jobId;
  } catch (err) {
    store.task = null;
    store.ui.progress = null;
    showError(err as Error);
  }
}

export function cancelCurrentJob() {
  const jobId = store.task?.jobId ?? store.ui.progress?.jobId;
  if (jobId) void api.cancelJob(jobId).catch(() => undefined);
}

// ---------- 同步对话框（推送 / 拉取 / 抓取，FR-5.2 扩展） ----------

let dialogJobId = 0;
let dialogDoneHandler: ((e: JobDoneEvent) => void) | null = null;

/** 同步对话框注册 job-done 处理（成功/失败/凭证重试均在对话框内展示）；传 null 注销 */
export function registerSyncDone(cb: ((e: JobDoneEvent) => void) | null) {
  dialogDoneHandler = cb;
}

export type SyncCreds = {
  kind: "http" | "ssh";
  username: string;
  password?: string;
  keyPath?: string;
  passphrase?: string;
};

export type SyncJobReq = {
  kind: "push" | "pull" | "fetch";
  /** push：多分支 refspec（remote 空串 = 同名） */
  refspecs?: { local: string; remote: string }[];
  /** pull：目标本地分支 */
  branch?: string;
  /** push：强制推送 */
  force?: boolean;
  creds: SyncCreds | null;
  remember: boolean;
};

/** 发起对话框同步任务（记 job_id 以便 job-done 分流到对话框） */
export async function startSyncJob(req: SyncJobReq): Promise<number> {
  const jobId = await api.syncStart({
    kind: req.kind,
    refspecs: req.refspecs ?? [],
    branch: req.branch ?? "",
    force: req.force ?? false,
    creds: req.creds,
    remember: req.remember,
  });
  dialogJobId = jobId;
  return jobId;
}

/** 打开同步对话框（工具栏 / 状态栏 / 右键菜单共用）；右键菜单传入仓库路径时先切换 */
export async function openSyncDialog(kind: SyncKind, path?: string) {
  if (path && store.repoPath !== path) {
    const { openRepo } = await import("./store");
    await openRepo(path);
  }
  if (!store.repoOpen) {
    toast(t("No repository open"));
    return;
  }
  store.ui.syncDialog = kind;
}

// ---------- 暂存 / 取消暂存 / 丢弃（FR-3） ----------

async function applyStatusUpdate(promise: Promise<StatusDto>) {
  try {
    store.status = await promise;
    await refreshFiles();
    void refreshRepos();
  } catch (e) {
    showError(e as Error);
  }
}

export async function stagePaths(paths: string[]) {
  await applyStatusUpdate(api.stagePaths(paths));
}

export async function unstagePaths(paths: string[]) {
  await applyStatusUpdate(api.unstagePaths(paths));
}

export async function stageAll() {
  const paths = changedRelPaths();
  if (paths.length) await stagePaths(paths);
}

export async function unstageAll() {
  const paths = stagedRelPaths();
  if (paths.length) await unstagePaths(paths);
}

/** 有变更（未暂存 / 未跟踪 / 冲突）的相对路径（含目录聚合行） */
export function changedRelPaths(): string[] {
  if (!store.status) return [];
  const out: string[] = [];
  for (const e of store.status.entries) {
    if (["modified", "untracked", "deleted", "renamed", "conflicted", "staged"].includes(e.kind)) {
      out.push(e.path);
    }
  }
  return out;
}

/** 已暂存的相对路径 */
export function stagedRelPaths(): string[] {
  if (!store.status) return [];
  return store.status.entries.filter((e) => e.kind === "staged").map((e) => e.path);
}

/** 丢弃更改（FR-3.4 二次确认） */
export async function discardPaths(paths: string[]) {
  if (paths.length === 0) return;
  const ok = await showConfirm({
    title: t("Discard Changes…"),
    message:
      paths.length === 1
        ? t("Discard this change? Uncommitted modifications will be lost permanently.")
        : t("Discard these changes? {count} files' uncommitted modifications will be lost permanently.", {
            count: paths.length,
          }),
    confirmLabel: t("Discard Changes…"),
    danger: true,
  });
  if (!ok) return;
  await applyStatusUpdate(api.discardPaths(paths));
}

// ---------- 提交（FR-4） ----------

/**
 * 提交（FR-4）。`prep` 为提交对话框勾选语义：提交前先把勾选的文件暂存
 * （未暂存内容纳入提交），再把未勾选但已暂存的文件反暂存（避免混入本次提交）；
 * `paths` 传入时按路径提交（git commit -- <paths> 语义），只提交这些文件。
 */
export async function doCommit(
  message: string,
  amend: boolean,
  prep?: { stage: string[]; unstage: string[] },
  paths?: string[],
): Promise<boolean> {
  if (!store.repoOpen) return false;
  try {
    if (prep?.unstage.length) await api.unstagePaths(prep.unstage);
    if (prep?.stage.length) await api.stagePaths(prep.stage);
    const shortId = await api.commit(message, amend, paths);
    toast(`${t("Commit created")} ${shortId}`);
    await refreshStatus();
    await refreshIdentity();
    return true;
  } catch (e) {
    if (e instanceof HigitApiError && e.code === "identity-missing") {
      store.ui.identityGuide = true;
    } else {
      showError(e as Error);
    }
    return false;
  }
}

/** 打开提交对话框（提交动作统一入口：工具栏 / 快捷键） */
export function openCommitDialog() {
  if (!store.repoOpen) {
    toast(t("No repository open"));
    return;
  }
  store.ui.commitDialog = true;
}

/** 错误码文案（供外部显示） */
export function errorText(code: string): string {
  return t(errorCodeKey(code));
}

/** 添加本地仓库（FR-1.4）：目录选择 + 校验 + 打开 */
export async function addLocalRepoAction() {
  const { openRepo, showError } = await import("./store");
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const dir = await open({ directory: true, title: t("Add Local Repository") });
    if (!dir || typeof dir !== "string") return;
    await api.addLocalRepo(dir);
    // openRepo 内部会刷新仓库列表；last_repo 由后端 open_repo 维护，
    // 不要走 persistConfig（前端 config 里的仓库列表是过期的，会把刚加的条目冲掉）
    await openRepo(dir);
  } catch (e) {
    showError(e as Error);
  }
}
