/** Tauri 命令封装（invoke）与事件监听 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  BootstrapDto,
  BranchDto,
  CommitDetailDto,
  CommitEntryDto,
  CommitMessageDto,
  CommitPreviewDto,
  ConfigDto,
  CredsDto,
  DiffDto,
  FileRowDto,
  IdentityDto,
  JobDoneEvent,
  JobProgressEvent,
  MergeOutcomeDto,
  SyncLogEvent,
  SyncPreviewDto,
  RepoEntryDto,
  RepoSummaryDto,
  RepoViewDto,
  StatusDto,
  StatusEvent,
  TreeRowDto,
} from "./types";

export class HigitApiError extends Error {
  code: string;
  params: string[];
  auth: { kind: "http" | "ssh"; host: string; username: string } | null;
  raw: string | null;

  constructor(e: { code: string; params: string[]; auth: unknown; raw: string | null }) {
    super(e.raw ?? e.code);
    this.code = e.code;
    this.params = e.params;
    this.auth = e.auth as never;
    this.raw = e.raw;
  }
}

export interface SyncReq {
  kind: "push" | "pull" | "fetch";
  branch: string;
  creds: CredsDto | null;
  remember: boolean;
}

export interface CloneReq {
  url: string;
  dest: string;
  creds: CredsDto | null;
  remember: boolean;
}

export const api = {
  getState: () => invoke<BootstrapDto>("get_state"),
  saveConfig: (config: ConfigDto) => invoke<void>("save_config", { config }),
  listRepos: () => invoke<RepoSummaryDto[]>("list_repos"),
  addLocalRepo: (path: string) => invoke<RepoEntryDto>("add_local_repo", { path }),
  removeRepo: (path: string) => invoke<void>("remove_repo", { path }),
  reorderRepos: (paths: string[]) => invoke<void>("reorder_repos", { paths }),
  openRepo: (path: string) => invoke<RepoViewDto>("open_repo", { path }),
  closeRepo: () => invoke<void>("close_repo"),
  destStatus: (path: string) => invoke<string>("dest_status", { path }),
  getRepoSettings: (id: string) => invoke<RepoEntryDto>("get_repo_settings", { id }),
  repoCredentialSaved: (id: string) => invoke<boolean>("repo_credential_saved", { id }),
  saveRepoSettings: (req: {
    id?: string | null;
    path: string;
    alias: string | null;
    auth: { kind: "http" | "ssh"; username: string; keyPath: string } | null;
    url: string;
    identity: IdentityDto | null;
    password?: string;
    rememberCred?: boolean;
  }) => invoke<void>("save_repo_settings", { req }),
  newRepo: (req: {
    parentDir: string;
    name: string;
    withReadme: boolean;
    withGitignore: boolean;
    gitignoreStack: string;
  }) => invoke<RepoViewDto>("new_repo", req),
  refreshStatus: () => invoke<StatusDto>("refresh_status"),
  listDir: (rel: string, showIgnored: boolean) =>
    invoke<FileRowDto[]>("list_dir", { rel, showIgnored }),
  listTree: (expanded: string[]) => invoke<TreeRowDto[]>("list_tree", { expanded }),
  stagePaths: (paths: string[]) => invoke<StatusDto>("stage_paths", { paths }),
  unstagePaths: (paths: string[]) => invoke<StatusDto>("unstage_paths", { paths }),
  discardPaths: (paths: string[]) => invoke<StatusDto>("discard_paths", { paths }),
  commit: (message: string, amend: boolean, paths?: string[]) =>
    invoke<string>("commit", { message, amend, paths: paths ?? null }),
  commitPreview: (rel: string) => invoke<CommitPreviewDto>("commit_preview", { rel }),
  lastCommitMessage: () => invoke<CommitMessageDto | null>("last_commit_message"),
  diffFile: (path: string, spec: "workdir" | "staged") =>
    invoke<DiffDto>("diff_file", { path, spec }),
  commitFileDiff: (oid: string, path: string) =>
    invoke<DiffDto>("commit_file_diff", { oid, path }),
  sync: (req: SyncReq) => invoke<number>("sync", { req }),
  syncPreview: () => invoke<SyncPreviewDto>("sync_preview"),
  syncStart: (req: {
    kind: "push" | "pull" | "fetch";
    refspecs?: { local: string; remote: string }[];
    branch?: string;
    force?: boolean;
    creds: CredsDto | null;
    remember: boolean;
  }) => invoke<number>("sync_start", { req }),
  cancelJob: (jobId: number) => invoke<void>("cancel_job", { jobId }),
  cloneRepo: (req: CloneReq) => invoke<number>("clone_repo", { req }),
  saveCredential: (host: string, creds: CredsDto) =>
    invoke<void>("save_credential", { host, creds }),
  historyPage: (limit: number, skip: number) =>
    invoke<CommitEntryDto[]>("history_page", { limit, skip }),
  commitDetail: (oid: string) => invoke<CommitDetailDto>("commit_detail", { oid }),
  listBranches: () => invoke<BranchDto[]>("list_branches"),
  branchCreate: (name: string, from: string, checkout: boolean) =>
    invoke<void>("branch_create", { name, from, checkout }),
  branchCheckout: (name: string) => invoke<StatusDto>("branch_checkout", { name }),
  branchRename: (oldName: string, newName: string) =>
    invoke<void>("branch_rename", { old: oldName, new: newName }),
  branchDelete: (name: string, force: boolean) =>
    invoke<void>("branch_delete", { name, force }),
  branchMerge: (name: string) => invoke<MergeOutcomeDto>("branch_merge", { name }),
  getEffectiveIdentity: () => invoke<IdentityDto | null>("get_effective_identity"),
  getGlobalIdentity: () => invoke<IdentityDto | null>("get_global_identity"),
  setIdentity: (scope: "repo" | "global", identity: IdentityDto) =>
    invoke<void>("set_identity", { scope, identity }),
  clearRepoIdentity: () => invoke<void>("clear_repo_identity"),
  openInExplorer: (path: string) => invoke<void>("open_in_explorer", { path }),
  openConfigDir: () => invoke<void>("open_config_dir"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  openInTerminal: () => invoke<void>("open_in_terminal"),
  editGitignore: () => invoke<void>("edit_gitignore"),
  exportDiagnostics: () => invoke<string>("export_diagnostics"),
};

export function onStatusUpdated(cb: (e: StatusEvent) => void): Promise<UnlistenFn> {
  return listen<StatusEvent>("status-updated", (ev) => cb(ev.payload));
}

export function onJobProgress(cb: (e: JobProgressEvent) => void): Promise<UnlistenFn> {
  return listen<JobProgressEvent>("job-progress", (ev) => cb(ev.payload));
}

export function onJobDone(cb: (e: JobDoneEvent) => void): Promise<UnlistenFn> {
  return listen<JobDoneEvent>("job-done", (ev) => cb(ev.payload));
}

// ---------- sync-log 事件总线（含早到缓冲回放） ----------
// 后端任务线程在 invoke 返回前就会发出首批日志（认证方式 / 出网方式 / 分支预览等），
// 前端此时还不知道 jobId，直接按 jobId 过滤会把这些行丢掉（表现为日志时有时无）。
// 因此这里统一接事件：先缓存，订阅者照常实时收到；发起方拿到 jobId 后调用
// replaySyncLogs 把该任务早到的日志按原顺序补发给订阅者（每个 jobId 至多回放一次）。

let syncLogSubs: ((e: SyncLogEvent) => void)[] = [];
let syncLogBuffer: SyncLogEvent[] = [];
const syncLogReplayed = new Set<number>();
let syncLogRawReady = false;

async function ensureSyncLogRaw(): Promise<void> {
  if (syncLogRawReady) return;
  syncLogRawReady = true;
  await listen<SyncLogEvent>("sync-log", (ev) => {
    const e = ev.payload;
    if (!syncLogReplayed.has(e.jobId)) {
      syncLogBuffer.push(e);
      if (syncLogBuffer.length > 200) syncLogBuffer.shift();
    }
    for (const cb of [...syncLogSubs]) cb(e);
  });
}

export async function onSyncLog(cb: (e: SyncLogEvent) => void): Promise<UnlistenFn> {
  await ensureSyncLogRaw();
  syncLogSubs.push(cb);
  return () => {
    syncLogSubs = syncLogSubs.filter((x) => x !== cb);
  };
}

/** 拿到 jobId 后调用：把早于该时刻到达的任务日志按原顺序补发（每 jobId 至多一次） */
export function replaySyncLogs(jobId: number): void {
  if (syncLogReplayed.has(jobId)) return;
  // 只记最近 50 个已回放的 jobId，防止长会话下集合无限增长
  syncLogReplayed.add(jobId);
  if (syncLogReplayed.size > 50) {
    const keep = [...syncLogReplayed].slice(-50);
    syncLogReplayed.clear();
    keep.forEach((id) => syncLogReplayed.add(id));
  }
  const early = syncLogBuffer.filter((e) => e.jobId === jobId);
  syncLogBuffer = syncLogBuffer.filter((e) => e.jobId !== jobId);
  for (const e of early) {
    for (const cb of [...syncLogSubs]) cb(e);
  }
}
