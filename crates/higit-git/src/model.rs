//! 引擎层数据模型：纯 Rust 类型，不含任何后端依赖。

use std::path::PathBuf;
use std::time::SystemTime;

/// 文件相对 HEAD 的 Git 状态（需求 §5.4 图例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatusKind {
    /// 未修改（与 HEAD 一致）
    Clean,
    /// 已修改（未暂存）
    Modified,
    /// 已暂存
    Staged,
    /// 未跟踪（新增）
    Untracked,
    /// 已删除
    Deleted,
    /// 重命名（P1）
    Renamed,
    /// 冲突
    Conflicted,
    /// 被 .gitignore 忽略
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub kind: FileStatusKind,
    /// 文件系统最后修改时间；目录或不可得时为 None
    pub mtime: Option<SystemTime>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub branch: Option<String>,
    pub entries: Vec<FileEntry>,
    /// 本地分支领先上游的提交数
    pub ahead: u32,
    /// 本地分支落后上游的提交数
    pub behind: u32,
    /// 是否设置了上游分支
    pub upstream: bool,
    /// HEAD 提交摘要（状态栏“最近提交”），空仓库为 None
    pub head: Option<HeadInfo>,
}

/// HEAD 提交信息（状态栏展示用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadInfo {
    /// 短 hash（7 位）
    pub short_id: String,
    /// 提交标题（首行）
    pub summary: String,
    /// 提交时间（Unix 秒，UTC）
    pub time_unix: i64,
    /// 提交者邮箱
    pub committer_email: String,
}

/// 差异查看范围（FR-8.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSpec {
    /// 工作区 vs 暂存区（未暂存改动）
    Workdir,
    /// HEAD vs 暂存区（已暂存改动）
    Staged,
}

/// git 身份写入范围（FR-4.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityScope {
    /// 仅当前仓库（.git/config）
    Repo,
    /// 全局（~/.gitconfig 等）
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineOrigin {
    Context,
    Addition,
    Deletion,
    HunkHeader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub origin: DiffLineOrigin,
    pub text: String,
}

/// 单文件文本差异（FR-8.1 / FR-8.4）。
#[derive(Debug, Clone, Default)]
pub struct TextDiff {
    pub lines: Vec<DiffLine>,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    /// 文本超过展示上限被截断
    pub truncated: bool,
    /// 文本文件超过 1MB，仅显示统计
    pub too_large: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Default)]
pub struct StatusOpts {
    /// 是否包含被 .gitignore 忽略的条目
    pub include_ignored: bool,
}

// ---------- M4：提交历史（FR-6） ----------

/// 提交历史列表条目（FR-6.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEntry {
    /// 完整 40 位 hash
    pub id: String,
    /// 短 hash（7 位）
    pub short_id: String,
    /// 提交标题（首行）
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// 提交时间（Unix 秒，UTC）
    pub time_unix: i64,
    /// 指向该提交的引用名（已去掉 refs/heads/ 等前缀；HEAD 所指形如 "HEAD -> main"）
    pub refs: Vec<String>,
    /// 父提交数（合并提交 > 1，供拓扑/详情使用）
    pub parents: usize,
}

/// 提交内变更文件的状态（FR-6.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitFileKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// 提交详情中的单个变更文件（FR-6.2 / FR-6.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: PathBuf,
    /// 重命名前的原路径（仅 Renamed）
    pub old_path: Option<PathBuf>,
    pub kind: CommitFileKind,
    pub additions: usize,
    pub deletions: usize,
}

/// 提交详情（FR-6.2）。
#[derive(Debug, Clone, Default)]
pub struct CommitDetail {
    pub id: String,
    pub short_id: String,
    /// 提交标题（首行）
    pub summary: String,
    /// 完整提交消息
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
    /// 提交时间（Unix 秒，UTC）
    pub time_unix: i64,
    /// 父提交完整 hash；根提交为空
    pub parents: Vec<String>,
    pub files: Vec<CommitFile>,
}

// ---------- M4：分支管理（FR-7） ----------

/// 分支信息（FR-7.1）；远端分支形如 "origin/main"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// 本地 "main" / 远端 "origin/main"
    pub name: String,
    pub is_remote: bool,
    /// 当前检出的分支
    pub is_current: bool,
    /// 目标提交短 hash
    pub short_id: String,
    /// 目标提交标题（首行备注）；远端分支为空
    pub last_commit_summary: String,
    /// 本地分支的上游名（如 "origin/main"）
    pub upstream: Option<String>,
    /// 领先 / 落后上游的提交数（无上游为 0）
    pub ahead: u32,
    pub behind: u32,
}

/// 合并结果（FR-7.4）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 是否以快进方式完成
    pub fast_forward: bool,
    /// 当前分支是否前进（false = 已是最新）
    pub updated: bool,
    /// 冲突文件列表（FR-5.8 P0）；非空表示合并被阻断、仓库处于冲突状态
    pub conflicts: Vec<PathBuf>,
}

/// 清理范围（git clean 的 -f / -fx / -fX 组合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanMode {
    /// 未跟踪 + 被忽略（-fx）
    All,
    /// 仅未跟踪（-f）
    UntrackedOnly,
    /// 仅被忽略（-fX）
    IgnoredOnly,
}

/// 工作区清理选项（git clean 语义）。
#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub mode: CleanMode,
    /// 同时清理未跟踪 / 被忽略目录（-d；false 时目录条目整体跳过，与 git clean 一致）
    pub dirs: bool,
    /// 移除包含 .git 的未管理目录（-ff；false 时此类目录视为嵌套仓库跳过）
    pub force_git_dirs: bool,
    /// 试运行：只收集将要清理的路径，不执行删除
    pub dry_run: bool,
    /// true = 直接删除；false = 移入系统回收站（默认，可从回收站恢复）
    pub no_recycle_bin: bool,
}

/// 工作区清理结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    /// 已清理（试运行 = 将被清理）的相对路径（正斜杠）
    pub paths: Vec<PathBuf>,
    /// 删除失败的路径及原因（试运行恒为空）
    pub failed: Vec<(PathBuf, String)>,
}
