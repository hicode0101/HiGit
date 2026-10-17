//! GitEngine trait：HiGit 的 Git 后端抽象。
//!
//! 约定：
//! - 所有方法对上层同步阻塞；耗时操作由调用方放到 worker 线程执行。
//! - trait 默认实现返回 `NotImplemented`，后端按里程碑逐个覆盖。
//! - `RepoSession` 对外不透明，内部按后端持有句柄；`git2::Repository` 等类型禁止越过本 crate。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{HigitError, Result};
use crate::model::{
    BranchInfo, CleanOptions, CleanOutcome, CommitDetail, CommitEntry, DiffSpec, HeadInfo,
    Identity, IdentityScope, MergeOutcome, RepoStatus, StatusOpts, TextDiff,
};

/// 长任务取消令牌。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 网络操作进度回调负载。
#[derive(Debug, Clone)]
pub struct Progress {
    /// 进度阶段（英文原文，作为 i18n 键，如 "Receiving objects"）
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
}

/// 打开的仓库会话；由哪个后端创建就只能传回给同一后端。
pub struct RepoSession {
    pub(crate) inner: SessionInner,
}

pub(crate) enum SessionInner {
    #[cfg(feature = "backend-git2")]
    Git2(crate::backend_git2::Git2Session),
}

/// 克隆请求（FR-1.2）。
#[derive(Debug, Clone)]
pub struct CloneRequest {
    /// HTTPS / SSH / 本地路径
    pub url: String,
    pub dest: PathBuf,
    /// 浅克隆深度；libgit2 不支持 shallow，暂只接受 None（FR-1.3 为 P1）
    pub depth: Option<u32>,
}

/// 网络凭证（FR-5.5 / FR-5.6）：clone / fetch / push / pull 共用。
/// SSH 的 `key_path` 为空表示依次尝试 ssh-agent 与 `~/.ssh` 默认密钥。
#[derive(Debug, Clone)]
pub enum Credentials {
    /// HTTPS 基础认证（用户名 + 密码 / Token）
    UserPass { username: String, password: String },
    /// SSH 私钥认证
    SshKey {
        username: String,
        key_path: String,
        passphrase: String,
        /// true = 来自系统凭证管理器的"记住"凭证（认证日志显示为"记住的密钥"，
        /// 区别于仓库设置里显式配置的"配置的密钥"）
        from_store: bool,
    },
}

/// 相等性只比凭证内容、忽略 `from_store` 来源标记：
/// 凭证重试链用它判断"已用凭证与记住的凭证相同则不再重试"。
impl PartialEq for Credentials {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::UserPass { username, password },
                Self::UserPass {
                    username: u2,
                    password: p2,
                },
            ) => username == u2 && password == p2,
            (
                Self::SshKey {
                    username,
                    key_path,
                    passphrase,
                    ..
                },
                Self::SshKey {
                    username: u2,
                    key_path: k2,
                    passphrase: p2,
                    ..
                },
            ) => username == u2 && key_path == k2 && passphrase == p2,
            _ => false,
        }
    }
}

impl Eq for Credentials {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_eq_ignores_from_store() {
        let explicit = Credentials::SshKey {
            username: "git".into(),
            key_path: "D:/keys/id".into(),
            passphrase: String::new(),
            from_store: false,
        };
        let stored = Credentials::SshKey {
            username: "git".into(),
            key_path: "D:/keys/id".into(),
            passphrase: String::new(),
            from_store: true,
        };
        // 内容相同、来源不同：视为同一凭证（重试链据此跳过无意义的重试）
        assert_eq!(explicit, stored);
        let different = Credentials::SshKey {
            username: "git".into(),
            key_path: "D:/keys/other".into(),
            passphrase: String::new(),
            from_store: true,
        };
        assert_ne!(explicit, different);
    }
}

/// push 结果（FR-5.2）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushOutcome {
    /// 本次是否为该分支新建立了上游（等价 `git push -u`）
    pub created_upstream: bool,
}

/// 推送请求（推送对话框，FR-5.2 扩展）：显式 refspec + force。
#[derive(Debug, Clone, Default)]
pub struct PushRequest {
    /// (本地分支, 远端分支)；远端分支空串 = 与本地同名
    pub refspecs: Vec<(String, String)>,
    /// 强制推送（等价 `--force`）：远端分支将被本地覆盖
    pub force: bool,
}

/// 同步过程事件（推送 / 拉取 / 抓取对话框的详细日志）。
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// 日志行；level: "info" | "ok" | "warn" | "error" | "remote" | "dim"
    Log { level: String, text: String },
    /// 传输进度（对话框进度条；stage 为 i18n 键）
    Progress {
        stage: &'static str,
        done: u64,
        total: u64,
    },
}

/// 抓取结果中单个远端引用的更新（抓取对话框日志）。
#[derive(Debug, Clone)]
pub struct FetchUpdate {
    /// 引用短名，如 "origin/main"
    pub name: String,
    /// 更新前短 hash（新分支为空）
    pub old_short: String,
    /// 更新后短 hash
    pub new_short: String,
}

/// 推送预览（推送对话框）：本地分支相对远端分支将上传的提交数与涉及文件。
#[derive(Debug, Clone, Default)]
pub struct PushPreview {
    pub commits: u32,
    pub files: Vec<String>,
}

/// pull 结果（FR-5.1，fetch + 快进合并）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullOutcome {
    /// 工作区是否前进到新提交
    pub updated: bool,
    /// 快进后的 HEAD（updated=true 时必有）
    pub head: Option<HeadInfo>,
}

/// 提交对话框的变更文件行：工作区全部变更（含未暂存），勾选即提交。
#[derive(Debug, Clone)]
pub struct ChangeFileInfo {
    /// 相对仓库根路径（正斜杠）
    pub path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "conflicted"
    pub kind: String,
    /// 该路径当前是否已有暂存变更（未勾选时需要反暂存，避免混入提交）
    pub staged: bool,
    /// 磁盘文件大小（已删除 / 不存在的文件为 None）
    pub size: Option<u64>,
    /// 磁盘最后修改时间（Unix 秒；已删除文件为 None）
    pub mtime_unix: Option<i64>,
}

/// 同步 Git 操作引擎（含网络长任务；实现方必须支持进度上报与取消，FR-5.10）。
pub trait GitEngine: Send + Sync {
    /// 在空目录创建新仓库（FR-1.1），默认分支为 main。
    fn init(&self, path: &Path) -> Result<RepoSession>;

    fn open(&self, workdir: &Path) -> Result<RepoSession>;

    fn status(&self, session: &RepoSession, opts: &StatusOpts) -> Result<RepoStatus>;

    /// 当前生效的 git 身份（user.name / user.email），未配置时返回 None。
    fn identity(&self, session: &RepoSession) -> Result<Option<Identity>>;

    // ---- 网络长任务（M1：clone；M3：fetch/push/pull，FR-5.10 进度 + 取消） ----

    fn clone_repo(
        &self,
        _req: &CloneRequest,
        _creds: Option<&Credentials>,
        _progress: &dyn Fn(Progress),
        _cancel: &CancelToken,
    ) -> Result<RepoSession> {
        Err(HigitError::not_implemented("clone_repo"))
    }

    /// 抓取远端（FR-5.3）。`remote` 为空时自动选择：上游所属远端 → origin → 第一个远端。
    fn fetch(
        &self,
        _session: &RepoSession,
        _remote: &str,
        _creds: Option<&Credentials>,
        _progress: &dyn Fn(Progress),
        _cancel: &CancelToken,
    ) -> Result<()> {
        Err(HigitError::not_implemented("fetch"))
    }

    /// 抓取远端所有分支（抓取对话框）：过程经 `on_event` 详细上报；返回更新的远端引用。
    fn fetch_verbose(
        &self,
        _session: &RepoSession,
        _remote: &str,
        _creds: Option<&Credentials>,
        _on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        _cancel: &CancelToken,
    ) -> Result<Vec<FetchUpdate>> {
        Err(HigitError::not_implemented("fetch_verbose"))
    }

    /// 推送指定本地分支（FR-5.2）：无上游时推到同名远端分支并建立 upstream（等价 `-u`）。
    fn push(
        &self,
        _session: &RepoSession,
        _branch: &str,
        _creds: Option<&Credentials>,
        _progress: &dyn Fn(Progress),
        _cancel: &CancelToken,
    ) -> Result<PushOutcome> {
        Err(HigitError::not_implemented("push"))
    }

    /// 多分支推送（推送对话框）：refspecs 显式给定，支持 force；
    /// 过程日志与进度经 `on_event` 详细上报（git 命令回显 / 远端输出 / 逐分支结果）。
    fn push_multi(
        &self,
        _session: &RepoSession,
        _req: &PushRequest,
        _creds: Option<&Credentials>,
        _on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        _cancel: &CancelToken,
    ) -> Result<PushOutcome> {
        Err(HigitError::not_implemented("push_multi"))
    }

    /// 解析同步远端（上游远端 → origin → 第一个远端），返回 (名称, URL)；仓库无远端时 None。
    fn sync_remote(&self, _session: &RepoSession) -> Result<Option<(String, String)>> {
        Err(HigitError::not_implemented("sync_remote"))
    }

    /// 推送预览（推送对话框）：本地分支将上传到远端分支的提交数与涉及文件；
    /// 远端分支不存在（新建分支）时为全部历史。
    fn push_preview(
        &self,
        _session: &RepoSession,
        _local: &str,
        _remote_branch: &str,
    ) -> Result<PushPreview> {
        Err(HigitError::not_implemented("push_preview"))
    }

    /// 拉取（FR-5.1）：fetch 指定本地分支的上游 + 快进合并该分支；
    /// 分叉时返回 `Diverged`（完整合并 UI 在后续里程碑）。
    fn pull(
        &self,
        _session: &RepoSession,
        _branch: &str,
        _creds: Option<&Credentials>,
        _progress: &dyn Fn(Progress),
        _cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        Err(HigitError::not_implemented("pull"))
    }

    /// 拉取（拉取对话框）：同 [`GitEngine::pull`]，过程经 `on_event` 详细上报。
    fn pull_verbose(
        &self,
        _session: &RepoSession,
        _branch: &str,
        _creds: Option<&Credentials>,
        _on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        _cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        Err(HigitError::not_implemented("pull_verbose"))
    }

    /// 设置网络代理（FR-10.4）：`None`/空串 = 直连；URL 原样交给 libgit2——Windows 上
    /// 经系统 WinHTTP 处理（`socks*://` 与常见代理软件不兼容，仅 HTTP 代理稳定可用），
    /// 非 Windows 的 libgit2 自带客户端无 SOCKS 实现。建议一律使用 `http(s)://` 代理地址。
    /// 会话无关，作用于其后的 clone / fetch / push / pull。
    fn set_proxy(&self, _proxy: Option<&str>) {}

    // ---- 以下为后续里程碑覆盖的写操作 / 查询 ----

    fn stage(&self, _session: &RepoSession, _paths: &[PathBuf]) -> Result<()> {
        Err(HigitError::not_implemented("stage"))
    }

    fn unstage(&self, _session: &RepoSession, _paths: &[PathBuf]) -> Result<()> {
        Err(HigitError::not_implemented("unstage"))
    }

    fn discard(&self, _session: &RepoSession, _paths: &[PathBuf]) -> Result<()> {
        Err(HigitError::not_implemented("discard"))
    }

    /// 工作区清理（git clean 语义）：列举未跟踪 / 被忽略条目并删除
    fn clean_worktree(&self, _session: &RepoSession, _opts: &CleanOptions) -> Result<CleanOutcome> {
        Err(HigitError::not_implemented("clean_worktree"))
    }

    fn commit(
        &self,
        _session: &RepoSession,
        _message: &str,
        _author: Option<&Identity>,
    ) -> Result<String> {
        Err(HigitError::not_implemented("commit"))
    }

    /// 修补上一次提交（FR-4.5）：以当前暂存区内容与给定消息替换 HEAD 提交。
    /// `author` 为空时保留原提交作者；unborn HEAD 时报错。
    fn commit_amend(
        &self,
        _session: &RepoSession,
        _message: &str,
        _author: Option<&Identity>,
    ) -> Result<String> {
        Err(HigitError::not_implemented("commit_amend"))
    }

    /// 提交预览（提交对话框）：工作区全部变更文件（含未暂存 / 未跟踪），
    /// 仅列 `dir`（相对仓库根，空 = 根目录）目录下的条目；
    /// 细化状态 + 是否已暂存 + 大小 + 修改时间；提交时按前端勾选自动暂存。
    fn commit_preview(&self, _session: &RepoSession, _dir: &str) -> Result<Vec<ChangeFileInfo>> {
        Err(HigitError::not_implemented("commit_preview"))
    }

    /// 按路径提交（提交对话框勾选语义）：勾选路径已由调用方先行暂存，
    /// 提交树 = HEAD 树 + 这些路径的暂存内容（等价 `git commit -- <paths>`），
    /// 其余变更（目录范围外 / 未勾选）不进入本次提交且暂存状态保持不变。
    fn commit_partial(
        &self,
        _session: &RepoSession,
        _message: &str,
        _author: Option<&Identity>,
        _paths: &[String],
    ) -> Result<String> {
        Err(HigitError::not_implemented("commit_partial"))
    }

    /// 单文件文本差异（FR-8.1）；路径相对仓库根。
    fn diff_text(&self, _session: &RepoSession, _path: &Path, _spec: DiffSpec) -> Result<TextDiff> {
        Err(HigitError::not_implemented("diff_text"))
    }

    /// 写入 git 身份（FR-4.2）；scope 决定写仓库级还是全局配置。
    fn set_identity(
        &self,
        _session: &RepoSession,
        _scope: IdentityScope,
        _identity: &Identity,
    ) -> Result<()> {
        Err(HigitError::not_implemented("set_identity"))
    }

    /// 读取全局身份（会话无关；设置页在未打开仓库时也可用，FR-10.3）。
    fn global_identity(&self) -> Result<Option<Identity>> {
        Err(HigitError::not_implemented("global_identity"))
    }

    /// 写入全局身份（会话无关）。
    fn set_global_identity(&self, _identity: &Identity) -> Result<()> {
        Err(HigitError::not_implemented("set_global_identity"))
    }

    /// 清除仓库级身份覆盖（仓库设置对话框“跟随全局”）：删除 .git/config 的 user.name / user.email。
    fn clear_repo_identity(&self, _session: &RepoSession) -> Result<()> {
        Err(HigitError::not_implemented("clear_repo_identity"))
    }

    /// 只读仓库级（.git/config）Git 身份；未在仓库级设置时为 None。
    /// 与 [`GitEngine::identity`]（合并配置、回落全局）不同，用于判断仓库是否单独配置过。
    fn local_identity(&self, _session: &RepoSession) -> Result<Option<Identity>> {
        Err(HigitError::not_implemented("local_identity"))
    }

    /// 读取远端 URL（仓库设置对话框）；远端不存在返回 None。
    fn remote_url(&self, _session: &RepoSession, _name: &str) -> Result<Option<String>> {
        Err(HigitError::not_implemented("remote_url"))
    }

    /// 写入远端 URL；远端不存在时创建。
    fn set_remote_url(&self, _session: &RepoSession, _name: &str, _url: &str) -> Result<()> {
        Err(HigitError::not_implemented("set_remote_url"))
    }

    // ---- M4：提交历史（FR-6） ----

    /// 提交历史（FR-6.1）：从 HEAD 按时间倒序取 `limit` 条；`skip` 用于滚动分页。
    fn log(&self, _session: &RepoSession, _limit: u32, _skip: u32) -> Result<Vec<CommitEntry>> {
        Err(HigitError::not_implemented("log"))
    }

    /// 提交详情（FR-6.2）：完整消息、作者/提交者、父提交、变更文件列表。
    fn commit_detail(&self, _session: &RepoSession, _oid: &str) -> Result<CommitDetail> {
        Err(HigitError::not_implemented("commit_detail"))
    }

    /// 提交内单文件差异（FR-6.3 / FR-8.2）：该提交 vs 第一父提交（根提交 vs 空树）。
    fn commit_file_diff(
        &self,
        _session: &RepoSession,
        _oid: &str,
        _path: &Path,
    ) -> Result<TextDiff> {
        Err(HigitError::not_implemented("commit_file_diff"))
    }

    // ---- M4：分支管理（FR-7） ----

    /// 分支列表（FR-7.1）：本地在前，远端随后；当前分支由 `is_current` 标记。
    fn branches(&self, _session: &RepoSession) -> Result<Vec<BranchInfo>> {
        Err(HigitError::not_implemented("branches"))
    }

    /// 新建分支（FR-7.2）。`from` 为空 = HEAD；否则解析为提交（hash / 分支名 / 表达式）。
    fn create_branch(
        &self,
        _session: &RepoSession,
        _name: &str,
        _from: Option<&str>,
    ) -> Result<()> {
        Err(HigitError::not_implemented("create_branch"))
    }

    /// 切换分支（FR-7.1/7.2）：安全检出，工作区修改冲突时返回 `DirtyWorktree`。
    fn checkout_branch(&self, _session: &RepoSession, _name: &str) -> Result<()> {
        Err(HigitError::not_implemented("checkout_branch"))
    }

    /// 重命名分支（FR-7.3）；若为当前分支 HEAD 自动跟随。
    fn rename_branch(&self, _session: &RepoSession, _old: &str, _new: &str) -> Result<()> {
        Err(HigitError::not_implemented("rename_branch"))
    }

    /// 删除分支（FR-7.3）。未合并且 `force=false` 时返回 `BranchNotMerged`（UI 强确认后重试）。
    fn delete_branch(&self, _session: &RepoSession, _name: &str, _force: bool) -> Result<()> {
        Err(HigitError::not_implemented("delete_branch"))
    }

    /// 合并到当前分支（FR-7.4）：优先快进，否则生成合并提交；
    /// 冲突时阻断并列出冲突文件（FR-5.8 P0），仓库保留冲突状态待解决后提交。
    fn merge_branch(&self, _session: &RepoSession, _name: &str) -> Result<MergeOutcome> {
        Err(HigitError::not_implemented("merge_branch"))
    }
}
