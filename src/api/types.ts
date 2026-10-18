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
  /** 相对 HEAD 的新增行数（二进制 / 文件过多未统计为 null） */
  additions: number | null;
  /** 相对 HEAD 的删除行数 */
  deletions: number | null;
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
  /** 目录下是否还有子目录（实心 / 描边文件夹图标区分） */
  hasChildren: boolean;
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

/** 增强合并请求（对应 TortoiseGit Merge 对话框选项） */
export interface MergeOptsDto {
  /** "branch" | "tag" | "commit" */
  kind: "branch" | "tag" | "commit";
  /** 分支名（本地或远端 "origin/x"）/ 标签名 / 提交 hash 或 revparse 表达式 */
  source: string;
  /** 禁用快进：总是产生合并提交（git merge --no-ff） */
  noFf: boolean;
  /** 仅允许快进：无法快进时报错（git merge --ff-only） */
  ffOnly: boolean;
  /** 合并后不自动创建合并提交（git merge --no-commit） */
  noCommit: boolean;
  /** 压缩合并（git merge --squash）：变更进入暂存区，不建合并提交 */
  squash: boolean;
  /** 合并提交消息：null/空 = 自动生成 */
  message: string | null;
  /** 自动消息附加被合并侧最近 N 条提交摘要（merge.log；0 = 不附加） */
  logMessages: number;
  /** 合并策略："" = 默认；"ours" = 结果树取当前分支（git merge -s ours） */
  strategy: string;
}

export interface MergeResultDto {
  fastForward: boolean;
  updated: boolean;
  conflicts: string[];
  /** 合并完成但未创建提交（--no-commit / --squash）：等待用户手动提交 */
  pendingCommit: boolean;
  /** 本次创建的合并提交 */
  createdCommit: string | null;
}

/** 拉取选项（对应 TortoiseGit Pull 对话框选项） */
export interface PullOptsDto {
  /** 变基整合（git pull --rebase） */
  rebase: boolean;
  /** 禁用快进（git pull --no-ff） */
  noFf: boolean;
  /** 仅允许快进（git pull --ff-only） */
  ffOnly: boolean;
  /** 合并后不自动提交（git pull --no-commit） */
  noCommit: boolean;
  /** 压缩合并（git pull --squash） */
  squash: boolean;
  /** 抓取所有标签（git fetch --tags） */
  allTags: boolean;
  /** 清理已被远端删除的远程跟踪分支（git fetch --prune） */
  prune: boolean;
}

export interface TagDto {
  name: string;
  shortId: string;
  lastCommitSummary: string;
}

/** 增强检出请求（对应 TortoiseGit Switch/Checkout 对话框选项） */
export interface CheckoutOptsDto {
  /** "branch" | "tag" | "commit" */
  kind: "branch" | "tag" | "commit";
  /** 分支名（本地或远端 "origin/x"）/ 标签名 / 提交 hash 或 revparse 表达式 */
  target: string;
  /** 在目标处新建分支并检出（分支名） */
  createBranch?: string | null;
  /** 强制检出：覆盖会被触及的工作区修改（git checkout -f） */
  force: boolean;
  /** 携带未提交修改切换（stash→检出→恢复，git checkout -m 语义） */
  merge: boolean;
  /** 目标为远端分支时建立跟踪（自动创建同名本地分支） */
  track: boolean;
  /** 新建/自动创建的本地分支已存在时强制移动到目标 */
  overrideExisting: boolean;
}

export interface CheckoutOutcomeDto {
  /** 是否处于分离 HEAD（检出标签/提交，或远端分支且未跟踪） */
  detached: boolean;
  /** 本次新建并切换到的本地分支名 */
  createdBranch: string | null;
  /** 携带的本地修改与目标分支冲突：冲突已写入工作区待解决 */
  conflicted: boolean;
}

/** 工作区清理请求（git clean 组合） */
export interface CleanOptsDto {
  /** "all" = -fx；"untracked" = -f；"ignored" = -fX */
  mode: "all" | "untracked" | "ignored";
  dirs: boolean;
  forceGitDirs: boolean;
  dryRun: boolean;
  noRecycleBin: boolean;
}

export interface CleanFailedDto {
  path: string;
  reason: string;
}

export interface CleanOutcomeDto {
  /** 已清理（试运行 = 将被清理）的相对路径 */
  paths: string[];
  failed: CleanFailedDto[];
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
  outcome: {
    createdUpstream?: boolean;
    updated?: boolean;
    headShortId?: string;
    fastForward?: boolean;
    rebased?: number;
    pendingCommit?: boolean;
    conflicts?: string[];
  } | null;
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

/** 同步对话框的远端选项（名称 + URL，推送对话框目标选择用） */
export interface SyncRemoteDto {
  name: string;
  url: string;
}

/** 同步对话框数据：仓库信息 + 远端 + 分支列表 */
export interface SyncPreviewDto {
  name: string;
  path: string;
  remoteName: string;
  url: string;
  /** 配置的所有远端 */
  remotes: SyncRemoteDto[];
  branches: SyncBranchDto[];
  remoteBranches: string[];
}

/** 同步日志事件（level: "info" | "ok" | "warn" | "error" | "remote" | "dim"） */
export interface SyncLogEvent {
  jobId: number;
  level: string;
  text: string;
}
