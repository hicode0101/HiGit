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
  saveRepoSettings: (req: {
    id?: string | null;
    path: string;
    alias: string | null;
    auth: { kind: "http" | "ssh"; username: string; keyPath: string } | null;
    url: string;
    identity: IdentityDto | null;
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

export function onSyncLog(cb: (e: SyncLogEvent) => void): Promise<UnlistenFn> {
  return listen<SyncLogEvent>("sync-log", (ev) => cb(ev.payload));
}
