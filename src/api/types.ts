/** 后端 DTO 类型（与 src-tauri/src/dto.rs 对应，camelCase） */

export type FileKind =
  | "clean"
  | "modified"
  | "staged"
  | "untracked"
  | "deleted"
  | "renamed"
  | "conflicted"
  | "ignored";

export interface EntryDto {
  path: string;
  kind: FileKind;
}

/** 提交对话框的变更文件行（kind: added | modified | deleted | renamed | conflicted） */
export interface StagedFileDto {
  path: string;
  kind: string;
  staged: boolean;
  size: number | null;
  mtimeUnix: number | null;
}

/** 提交对话框预览数据（committer 为 "name <email>"，未配置时为空串） */
export interface CommitPreviewDto {
  name: string;
  path: string;
  committer: string;
  files: StagedFileDto[];
}

export interface HeadDto {
  shortId: string;
  summary: string;
  timeUnix: number;
  committerEmail: string;
}

export interface StatusDto {
  branch: string | null;
  ahead: number;
  behind: number;
  upstream: boolean;
  head: HeadDto | null;
  entries: EntryDto[];
}

export interface FileRowDto {
  name: string;
  relPath: string;
  absPath: string;
  isDir: boolean;
  kind: FileKind;
  size: number | null;
  mtimeUnix: number | null;
}

export interface TreeRowDto {
  absPath: string;
  name: string;
  level: number;
  gitMeta: boolean;
}

export interface RepoSummaryDto {
  id: string;
  name: string;
  path: string;
  alias: string | null;
  branch: string | null;
  dirty: number;
}

export interface IdentityDto {
  name: string;
  email: string;
}

export interface AuthCfgDto {
  kind: "http" | "ssh";
  username: string;
  keyPath: string;
}

export interface RepoEntryDto {
  id: string;
  name: string;
  path: string;
  alias: string | null;
  auth: AuthCfgDto | null;
  url: string;
  identity: IdentityDto | null;
}

export interface ConfigDto {
  language: string;
  theme: string;
  recentRepos: RepoEntryDto[];
  lastRepo: string;
  autoRefresh: boolean;
  showIgnored: boolean;
  gitBackend: string;
  proxyEnabled: boolean;
  proxy: string;
}

export interface BootstrapDto {
  config: ConfigDto;
  systemTheme: "light" | "dark";
  systemLanguage: string;
  version: string;
  configCorrupted: boolean;
}

export interface DiffLineDto {
  origin: "context" | "add" | "del" | "hunk";
  text: string;
}

export interface DiffDto {
  lines: DiffLineDto[];
  additions: number;
  deletions: number;
  isBinary: boolean;
  truncated: boolean;
  tooLarge: boolean;
}

export interface CommitEntryDto {
  id: string;
  shortId: string;
  summary: string;
  authorName: string;
  authorEmail: string;
  timeUnix: number;
  refs: string[];
  parents: number;
}

export interface CommitFileDto {
  path: string;
  oldPath: string | null;
  kind: "added" | "modified" | "deleted" | "renamed";
  additions: number;
  deletions: number;
}

export interface CommitDetailDto {
  id: string;
  shortId: string;
  summary: string;
  message: string;
  authorName: string;
  authorEmail: string;
  committerName: string;
  committerEmail: string;
  timeUnix: number;
  parents: string[];
  files: CommitFileDto[];
}

export interface BranchDto {
  name: string;
  isRemote: boolean;
  isCurrent: boolean;
  shortId: string;
  lastCommitSummary: string;
  upstream: string | null;
  ahead: number;
  behind: number;
}

export interface MergeOutcomeDto {
  fastForward: boolean;
  updated: boolean;
  conflicts: string[];
}

export interface CredsDto {
  kind: "http" | "ssh";
  username: string;
  password?: string;
  keyPath?: string;
  passphrase?: string;
}

export interface RepoViewDto {
  path: string;
  name: string;
  status: StatusDto;
}

export interface CommitMessageDto {
  summary: string;
  body: string;
}

/** 结构化错误（src-tauri/src/error.rs CommandError） */
export interface AuthDetailDto {
  kind: "http" | "ssh";
  host: string;
  username: string;
}

export interface CommandErrorDto {
  code: string;
  params: string[];
  auth: AuthDetailDto | null;
  raw: string | null;
}

export interface JobProgressEvent {
  jobId: number;
  stage: string;
  done: number;
  total: number;
}

export interface JobDoneEvent {
  jobId: number;
  kind: "clone" | "push" | "pull" | "fetch";
  ok: boolean;
  error: CommandErrorDto | null;
  repoPath: string | null;
  outcome: { createdUpstream?: boolean; updated?: boolean; headShortId?: string } | null;
}

export interface StatusEvent {
  path: string;
  status: StatusDto;
}

/** 同步对话框（推送/拉取/抓取）：本地分支行 */
export interface SyncBranchDto {
  name: string;
  isCurrent: boolean;
  shortId: string;
  summary: string;
  upstream: string | null;
  ahead: number;
  behind: number;
}

/** 同步对话框数据：仓库信息 + 远端 + 分支列表 */
export interface SyncPreviewDto {
  name: string;
  path: string;
  remoteName: string;
  url: string;
  branches: SyncBranchDto[];
  remoteBranches: string[];
}

/** 同步日志事件（level: "info" | "ok" | "warn" | "error" | "remote" | "dim"） */
export interface SyncLogEvent {
  jobId: number;
  level: string;
  text: string;
}
