//! git2（libgit2）后端实现。
//!
//! M0：init / open / status / identity / stage / commit；
//! M1：status 附带 HEAD 摘要与 ahead/behind；NetworkOps::clone_repo；
//! M2：unstage / discard / diff_text / set_identity；
//! M3：fetch / push / pull（凭证回调 + 进度 + 取消）与全局身份读写；
//! M4：log / commit_detail / commit_file_diff（FR-6），
//!     branches / create / checkout / rename / delete / merge（FR-7），
//!     提交识别 MERGE_HEAD（冲突解决后可落合并提交，FR-5.8 P0）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use git2::{
    BranchType, Cred, CredentialType, Delta, FetchPrune, Oid, PackBuilderStage, PushOptions,
    Repository, Signature, Sort, Status,
};

/// diff 文本展示上限：单文件 > 1MB 仅显示统计（FR-8.4）
const DIFF_TEXT_MAX_BYTES: u64 = 1024 * 1024;
/// diff 行数展示上限（超出截断，FR-8.4）
const DIFF_LINE_CAP: usize = 4000;
/// 提交对话框行级统计的文件数上限：超过则整体跳过（性能保护）
const COMMIT_STATS_MAX_FILES: usize = 200;

use crate::engine::{
    CancelToken, ChangeFileInfo, CloneRequest, Credentials, FetchUpdate, GitEngine, Progress,
    PullOptions, PullOutcome, PushRequest, PushPreview, PushOutcome, RepoSession, SessionInner,
    SyncEvent,
};
use crate::error::{AuthDetail, AuthKind, ErrorCode, HigitError, Result};
use crate::model::{
    BranchInfo, CheckoutOptions, CheckoutOutcome, CheckoutTargetKind, CleanMode, CleanOptions,
    CleanOutcome, CommitDetail, CommitEntry, CommitFile, CommitFileKind, DiffLine, DiffLineOrigin,
    DiffSpec, FileEntry, FileStatusKind, HeadInfo, Identity, IdentityScope, MergeOptions,
    MergeOutcome, MergeResult, RepoStatus, StatusOpts, TagInfo, TextDiff,
};

mod ssh;

pub struct Git2Backend {
    /// 网络代理 URL（FR-10.4）：None/空 = 直连；作用于 clone/fetch/push/pull
    proxy: Mutex<Option<String>>,
}

impl Git2Backend {
    pub fn new() -> Self {
        Self {
            proxy: Mutex::new(None),
        }
    }

    /// 代理选项；未设置时返回 None（保持 libgit2 默认行为）。
    fn proxy_opts(&self) -> Option<git2::ProxyOptions<'static>> {
        let guard = self.proxy.lock().expect("proxy mutex poisoned");
        guard.as_ref().map(|url| {
            let mut o = git2::ProxyOptions::new();
            o.url(url);
            o
        })
    }

    fn with_session<T>(
        &self,
        session: &RepoSession,
        f: impl FnOnce(&Git2Session, &Repository) -> Result<T>,
    ) -> Result<T> {
        let SessionInner::Git2(s) = &session.inner;
        let repo = s.repo.lock().expect("git2 session mutex poisoned");
        f(s, &repo)
    }

    /// 同 with_session，但提供 `&mut Repository`（stash 等 libgit2 可变 API 需要）。
    fn with_session_mut<T>(
        &self,
        session: &RepoSession,
        f: impl FnOnce(&Git2Session, &mut Repository) -> Result<T>,
    ) -> Result<T> {
        let SessionInner::Git2(s) = &session.inner;
        let mut repo = s.repo.lock().expect("git2 session mutex poisoned");
        f(s, &mut repo)
    }

    /// 网络回调（详细日志版）：进度可路由到简单 progress 或 [`SyncEvent`]；
    /// 远端 sideband 输出按行上报为日志；凭证链与主机密钥校验同 [`build_callbacks`]。
    /// `auth` 提供时把凭证链每一步的尝试 / 结果上报为同步日志。
    fn build_verbose_callbacks<'a>(
        &self,
        creds: Option<&Credentials>,
        url: &str,
        progress: Option<&'a dyn Fn(Progress)>,
        on_event: Option<&'a (dyn Fn(SyncEvent) + Send + Sync)>,
        auth: Option<&'a AuthLog<'a>>,
        cancel: &CancelToken,
    ) -> git2::RemoteCallbacks<'a> {
        let mut cbs = git2::RemoteCallbacks::new();
        {
            let cancel = cancel.clone();
            cbs.transfer_progress(move |stats| {
                let (stage, done, total) = if stats.received_objects() < stats.total_objects() {
                    (
                        "Receiving objects",
                        stats.received_objects() as u64,
                        stats.total_objects() as u64,
                    )
                } else {
                    (
                        "Resolving deltas",
                        stats.indexed_deltas() as u64,
                        stats.total_deltas() as u64,
                    )
                };
                match (progress, on_event) {
                    (_, Some(ev)) => ev(SyncEvent::Progress { stage, done, total }),
                    (Some(p), None) => p(Progress { stage, done, total }),
                    _ => {}
                }
                !cancel.is_cancelled()
            });
        }
        if let Some(ev) = on_event {
            let mut buf = String::new();
            let cancel = cancel.clone();
            cbs.sideband_progress(move |data| {
                if cancel.is_cancelled() {
                    return false;
                }
                buf.push_str(&String::from_utf8_lossy(data));
                while let Some(idx) = buf.find('\n') {
                    let line: String = buf.drain(..=idx).collect();
                    let line = line.trim_end_matches(['\n', '\r']);
                    if !line.is_empty() {
                        ev(SyncEvent::Log {
                            level: "remote".into(),
                            text: line.to_string(),
                        });
                    }
                }
                true
            });
        } else {
            let cancel = cancel.clone();
            cbs.sideband_progress(move |_data| !cancel.is_cancelled());
        }
        apply_credentials(
            &mut cbs,
            creds.cloned().unwrap_or(Credentials::SshKey {
                username: String::new(),
                key_path: String::new(),
                passphrase: String::new(),
                from_store: false,
            }),
            auth,
        );
        set_hostkey_callback(&mut cbs, url);
        cbs
    }

    /// 远端引用快照（refs/remotes/<remote>/* → tip），抓取前后对比得出更新列表。
    fn remote_ref_snapshot(repo: &Repository, remote_name: &str) -> HashMap<String, Oid> {
        let prefix = format!("refs/remotes/{remote_name}/");
        let mut out = HashMap::new();
        if let Ok(iter) = repo.references() {
            for r in iter.flatten() {
                let Some(name) = r.name().ok().map(str::to_string) else {
                    continue;
                };
                if !name.starts_with(&prefix) {
                    continue;
                }
                if let Some(tip) = r.target() {
                    out.insert(name, tip);
                }
            }
        }
        out
    }

    /// 抓取实现：plain fetch（progress 回调）与 fetch_verbose（详细日志）共用。
    fn fetch_impl(
        &self,
        repo: &Repository,
        remote: &str,
        creds: Option<&Credentials>,
        progress: Option<&dyn Fn(Progress)>,
        on_event: Option<&(dyn Fn(SyncEvent) + Send + Sync)>,
        cancel: &CancelToken,
    ) -> Result<Vec<FetchUpdate>> {
        let branch = current_branch(repo);
        let name = resolve_remote_name(repo, remote, branch.as_deref())?;
        let mut r = repo
            .find_remote(&name)
            .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?;
        let url = r.url().unwrap_or_default().to_string();

        let log = |level: &str, text: String| {
            if let Some(ev) = on_event {
                ev(SyncEvent::Log {
                    level: level.to_string(),
                    text,
                });
            }
        };
        log("dim", format!("git fetch --progress \"{name}\""));
        log("dim", format!("远端: {url}"));

        let before = Self::remote_ref_snapshot(repo, &name);
        let auth = on_event.map(AuthLog::new);
        let cbs = self.build_verbose_callbacks(creds, &url, progress, on_event, auth.as_ref(), cancel);
        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(cbs);
        opts.prune(FetchPrune::On);
        if let Some(px) = self.proxy_opts() {
            opts.proxy_options(px);
        }
        // 空切片 = 使用 remote.<name>.fetch 已配置的 refspec
        r.fetch(&[] as &[&str], Some(&mut opts), None)
            .map_err(|e| map_net_err(e, &url, cancel.is_cancelled(), creds.is_some()))?;
        if let Some(a) = auth.as_ref() {
            a.succeed();
        }

        let after = Self::remote_ref_snapshot(repo, &name);
        let mut updates = Vec::new();
        for (name, tip) in &after {
            let short_name = name.trim_start_matches("refs/remotes/").to_string();
            match before.get(name) {
                None => updates.push(FetchUpdate {
                    name: short_name,
                    old_short: String::new(),
                    new_short: short_id(*tip),
                }),
                Some(old) if old != tip => updates.push(FetchUpdate {
                    name: short_name,
                    old_short: short_id(*old),
                    new_short: short_id(*tip),
                }),
                _ => {}
            }
        }
        updates.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(updates)
    }

    /// 拉取实现：plain pull 与 pull_verbose 共用（on_event 有值时输出详细日志）。
    /// 拉取第 1 阶段：解析上游 → 抓取（可带 --prune / --tags）→ 计算本地/远端关系，
    /// 决定整合方案（快进 / 合并 / 变基 / 已最新 / 分叉报错）。
    #[allow(clippy::too_many_arguments)]
    fn pull_fetch_plan(
        &self,
        repo: &Repository,
        branch: &str,
        opts: &PullOptions,
        creds: Option<&Credentials>,
        progress: Option<&dyn Fn(Progress)>,
        on_event: Option<&(dyn Fn(SyncEvent) + Send + Sync)>,
        cancel: &CancelToken,
    ) -> Result<PullPlan> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(HigitError::new(ErrorCode::Unknown, "no branch to pull"));
        }
        let log = sync_logger(on_event);
        let local = repo
            .find_branch(branch, git2::BranchType::Local)
            .map_err(|_| {
                HigitError::new(ErrorCode::Unknown, format!("branch not found: {branch}"))
            })?;
        let upstream = local.upstream().map_err(|_| {
            HigitError::new(
                ErrorCode::NoUpstream,
                format!("分支 {branch} 未设置上游，无法拉取；请先推送一次（-u 建立上游）或在仓库设置中配置"),
            )
        })?;
        let up_name = upstream
            .name()
            .ok()
            .flatten()
            .ok_or_else(|| {
                HigitError::new(
                    ErrorCode::NoUpstream,
                    format!("分支 {branch} 的上游引用名无法解析，无法拉取"),
                )
            })?;
        let (remote_name, remote_branch) = up_name
            .split_once('/')
            .ok_or_else(|| {
                HigitError::new(
                    ErrorCode::NoUpstream,
                    format!("上游 {up_name} 不是有效的 <远端>/<分支> 形式，无法拉取"),
                )
            })?;

        // 仅抓取当前分支的上游 refspec（可附 --tags）
        let mut r = repo
            .find_remote(remote_name)
            .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?;
        let url = r.url().unwrap_or_default().to_string();
        log(
            "dim",
            format!("git pull --progress \"{remote_name}\" {remote_branch}"),
        );
        log("dim", format!("远端: {url}"));
        let auth = on_event.map(AuthLog::new);
        let cbs = self.build_verbose_callbacks(creds, &url, progress, on_event, auth.as_ref(), cancel);
        let mut fopts = git2::FetchOptions::new();
        fopts.remote_callbacks(cbs);
        if let Some(px) = self.proxy_opts() {
            fopts.proxy_options(px);
        }
        if opts.prune {
            fopts.prune(FetchPrune::On);
            log("dim", "fetch --prune".into());
        }
        let mut specs = vec![format!(
            "+refs/heads/{remote_branch}:refs/remotes/{remote_name}/{remote_branch}"
        )];
        if opts.prune {
            // prune 生效范围覆盖整个远端：同时刷新全部远端跟踪引用，
            // 清理所有已被远端删除的分支（仅限当前分支 refspec 时 prune 无内容可清理）
            specs.push(format!("+refs/heads/*:refs/remotes/{remote_name}/*"));
        }
        if opts.all_tags {
            specs.push("+refs/tags/*:refs/tags/*".to_string());
            log("dim", "fetch --tags".into());
        }
        r.fetch(&specs, Some(&mut fopts), None)
            .map_err(|e| map_net_err(e, &url, cancel.is_cancelled(), creds.is_some()))?;
        if let Some(a) = auth.as_ref() {
            a.succeed();
        }

        let local_oid = local
            .get()
            .target()
            .ok_or_else(|| HigitError::new(ErrorCode::Unknown, "unborn branch"))?;
        let up_ref = repo
            .find_reference(&format!("refs/remotes/{remote_name}/{remote_branch}"))
            .map_err(|e| HigitError::new(ErrorCode::NoUpstream, e.message()))?;
        let Some(up_oid) = up_ref.target() else {
            return Ok(PullPlan::UpToDate);
        };
        if local_oid == up_oid {
            return Ok(PullPlan::UpToDate);
        }
        let (ahead, behind) = repo
            .graph_ahead_behind(local_oid, up_oid)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        if behind == 0 {
            // 本地领先或对等：无需更新
            return Ok(PullPlan::UpToDate);
        }
        let ff_able = ahead == 0;
        if opts.ff_only && !ff_able {
            return Err(HigitError::new(
                ErrorCode::Diverged,
                format!(
                    "{branch} 与 {up_name} 已分叉：本地领先 {ahead} 个提交、落后 {behind} 个提交，无法快进；请先处理分叉（合并远端提交或变基）后重试"
                ),
            ));
        }
        if ff_able {
            // no_ff / squash 需要产生合并动作（squash 不移动分支引用）
            if opts.no_ff || opts.squash {
                return Ok(PullPlan::Merge { up_name: up_name.to_string() });
            }
            return Ok(PullPlan::FastForward { up_oid, up_name: up_name.to_string() });
        }
        if opts.rebase {
            return Ok(PullPlan::Rebase {
                up_name: up_name.to_string(),
                up_oid,
            });
        }
        Ok(PullPlan::Merge {
            up_name: up_name.to_string(),
        })
    }

    /// 拉取第 2 阶段（快进）：当前分支先安全 checkout（不覆盖未提交修改）→ 前移分支引用；
    /// 非当前分支仅前移引用（无工作区变更）。
    fn pull_fast_forward(
        &self,
        repo: &Repository,
        branch: &str,
        up_oid: Oid,
        up_name: &str,
        log: &dyn Fn(&str, String),
    ) -> Result<PullOutcome> {
        let commit = repo
            .find_commit(up_oid)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        log(
            "info",
            format!(
                "fast-forward {branch} → {}",
                up_oid.to_string().chars().take(7).collect::<String>()
            ),
        );
        if repo
            .find_branch(branch, git2::BranchType::Local)
            .map(|b| b.is_head())
            .unwrap_or(false)
        {
            repo.checkout_tree(commit.as_object(), None).map_err(|e| {
                HigitError::new(
                    ErrorCode::DirtyWorktree,
                    format!("checkout would overwrite local changes: {}", e.message()),
                )
            })?;
        }
        let mut local_ref = repo
            .find_reference(&format!("refs/heads/{branch}"))
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        local_ref
            .set_target(up_oid, &format!("pull: fast-forward to {up_name}"))
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

        Ok(PullOutcome {
            updated: true,
            head: Some(head_info_of(repo)?),
            fast_forward: true,
            ..Default::default()
        })
    }

    /// 拉取第 2 阶段（变基，git pull --rebase）：把本地提交重放到远端最新提交之上。
    /// 冲突时中止变基并恢复原状（轻量客户端不提供交互式变基，错误中说明）。
    fn pull_rebase(
        &self,
        repo: &Repository,
        branch: &str,
        up_name: &str,
        up_oid: Oid,
        log: &dyn Fn(&str, String),
    ) -> Result<PullOutcome> {
        let sig = merge_signature(repo)?;
        let onto = repo
            .find_annotated_commit(up_oid)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        log(
            "info",
            format!(
                "rebase {branch} onto {up_name} ({})",
                up_oid.to_string().chars().take(7).collect::<String>()
            ),
        );
        let mut rb = repo
            .rebase(None, Some(&onto), None, None)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        let mut rebased: u32 = 0;
        loop {
            match rb.next() {
                Some(Ok(_op)) => {
                    // libgit2 应用冲突时 next() 可能不报错，而是把冲突写进索引
                    let conflicted = repo.index().map(|i| i.has_conflicts()).unwrap_or(false);
                    if conflicted {
                        let _ = rb.abort();
                        return Err(HigitError::new(
                            ErrorCode::MergeConflict,
                            format!(
                                "rebase conflict while replaying commits onto {up_name}; rebase aborted and original branch restored"
                            ),
                        ));
                    }
                    match rb.commit(None, &sig, None) {
                        // author / message 传 None = 沿用原提交
                        Ok(_) => rebased += 1,
                        Err(e) => {
                            let _ = rb.abort();
                            return Err(HigitError::new(
                                ErrorCode::Unknown,
                                format!("rebase failed at commit {rebased}: {}; original branch restored", e.message()),
                            ));
                        }
                    }
                }
                Some(Err(e)) => {
                    // 冲突：中止变基恢复原状，提示改用合并或手动变基
                    let _ = rb.abort();
                    return Err(HigitError::new(
                        ErrorCode::MergeConflict,
                        format!(
                            "rebase conflict while replaying commits onto {up_name}: {}; rebase aborted and original branch restored",
                            e.message()
                        ),
                    ));
                }
                None => break,
            }
        }
        rb.finish(Some(&sig))
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        log("ok", format!("rebased {rebased} commit(s) onto {up_name}"));
        Ok(PullOutcome {
            updated: true,
            head: Some(head_info_of(repo)?),
            fast_forward: false,
            rebased,
            pending_commit: false,
            conflicts: Vec::new(),
        })
    }
}

impl Default for Git2Backend {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Git2Session {
    repo: Mutex<Repository>,
    workdir: PathBuf,
}

impl GitEngine for Git2Backend {
    fn init(&self, path: &Path) -> Result<RepoSession> {
        let repo =
            Repository::init(path).map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        // 统一默认分支为 main（现代约定，与原型一致）
        let _ = repo.set_head("refs/heads/main");
        let workdir = repo
            .workdir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf());
        Ok(RepoSession {
            inner: SessionInner::Git2(Git2Session {
                repo: Mutex::new(repo),
                workdir,
            }),
        })
    }

    fn open(&self, workdir: &Path) -> Result<RepoSession> {
        let path = workdir.to_string_lossy().to_string();
        let repo = Repository::discover(workdir)
            .map_err(|e| HigitError::invalid_repository(path.clone(), e.message()))?;
        let workdir = repo
            .workdir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| workdir.to_path_buf());
        Ok(RepoSession {
            inner: SessionInner::Git2(Git2Session {
                repo: Mutex::new(repo),
                workdir,
            }),
        })
    }

    fn status(&self, session: &RepoSession, opts: &StatusOpts) -> Result<RepoStatus> {
        self.with_session(session, |s, repo| {
            let branch = match repo
                .head()
                .ok()
                .and_then(|head| head.shorthand().ok().map(|s| s.to_string()))
            {
                Some(b) => Some(b),
                None => {
                    // 空仓库 HEAD 未出生：从 HEAD 符号引用回退读取目标分支名
                    repo.find_reference("HEAD")
                        .ok()
                        .and_then(|r| {
                            r.symbolic_target()
                                .ok()
                                .flatten()
                                .map(|t| t.trim_start_matches("refs/heads/").to_string())
                        })
                        .filter(|s| !s.is_empty())
                }
            };

            let mut status_opts = git2::StatusOptions::new();
            status_opts
                .include_untracked(true)
                .include_ignored(opts.include_ignored);
            let statuses = repo
                .statuses(Some(&mut status_opts))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;

            let mut entries = Vec::with_capacity(statuses.len());
            for item in statuses.iter() {
                let Some(kind) = map_status(item.status(), opts.include_ignored) else {
                    continue;
                };
                let Some(path) = item.path().ok() else {
                    continue;
                };
                // 未跟踪目录形如 "dir/"：统一去掉尾部斜杠，保证与文件系统路径可匹配
                let path = PathBuf::from(path.trim_end_matches('/'));
                let mtime = std::fs::metadata(s.workdir.join(&path))
                    .and_then(|m| m.modified())
                    .ok();
                entries.push(FileEntry { path, kind, mtime });
            }
            entries.sort_by(|a, b| a.path.cmp(&b.path));

            // HEAD 摘要与 ahead/behind（状态栏展示，失败不阻断 status）
            let mut head = None;
            let mut ahead = 0u32;
            let mut behind = 0u32;
            let mut has_upstream = false;
            if let Ok(head_ref) = repo.head() {
                if let Ok(commit) = head_ref.peel_to_commit() {
                    head = Some(HeadInfo {
                        short_id: commit.id().to_string().chars().take(7).collect(),
                        summary: commit
                            .summary()
                            .ok()
                            .flatten()
                            .map(str::to_string)
                            .unwrap_or_default(),
                        time_unix: commit.time().seconds(),
                        committer_email: commit.committer().email().unwrap_or("").to_string(),
                    });
                    if let Some(name) = branch.as_deref() {
                        if let Ok(local) = repo.find_branch(name, git2::BranchType::Local) {
                            if let Ok(upstream) = local.upstream() {
                                has_upstream = true;
                                if let Ok(up_commit) = upstream.get().peel_to_commit() {
                                    if let Ok((a, b)) =
                                        repo.graph_ahead_behind(commit.id(), up_commit.id())
                                    {
                                        ahead = a as u32;
                                        behind = b as u32;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(RepoStatus {
                branch,
                entries,
                ahead,
                behind,
                upstream: has_upstream,
                head,
            })
        })
    }

    fn identity(&self, session: &RepoSession) -> Result<Option<Identity>> {
        self.with_session(session, |_s, repo| {
            let Ok(config) = repo.config() else {
                return Ok(None);
            };

            let name = config.get_string("user.name").ok();
            let email = config.get_string("user.email").ok();
            match (name, email) {
                (Some(name), Some(email)) if !name.is_empty() && !email.is_empty() => {
                    Ok(Some(Identity { name, email }))
                }
                _ => Ok(None),
            }
        })
    }

    /// 克隆仓库（FR-1.2）：进度经回调上报，取消经 [`CancelToken`] 注入。
    /// 凭证支持 HTTPS 用户名/密码 与 SSH 密钥（FR-5.5/5.6）。
    fn clone_repo(
        &self,
        req: &CloneRequest,
        creds: Option<&Credentials>,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<RepoSession> {
        let cbs = build_callbacks(creds, &req.url, progress, cancel);

        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(cbs);
        if let Some(px) = self.proxy_opts() {
            fetch_opts.proxy_options(px);
        }
        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fetch_opts);

        let repo = builder
            .clone(&req.url, &req.dest)
            .map_err(|e| map_net_err(e, &req.url, cancel.is_cancelled(), creds.is_some()))?;

        let workdir = repo
            .workdir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| req.dest.clone());
        Ok(RepoSession {
            inner: SessionInner::Git2(Git2Session {
                repo: Mutex::new(repo),
                workdir,
            }),
        })
    }

    /// 抓取（FR-5.3）：按远端配置的 refspec 抓取全部分支。
    fn fetch(
        &self,
        session: &RepoSession,
        remote: &str,
        creds: Option<&Credentials>,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<()> {
        self.with_session(session, |_s, repo| {
            self.fetch_impl(repo, remote, creds, Some(progress), None, cancel)?;
            Ok(())
        })
    }

    /// 抓取远端所有分支（抓取对话框）：过程经 `on_event` 详细上报；返回更新的远端引用。
    fn fetch_verbose(
        &self,
        session: &RepoSession,
        remote: &str,
        creds: Option<&Credentials>,
        on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        cancel: &CancelToken,
    ) -> Result<Vec<FetchUpdate>> {
        self.with_session(session, |_s, repo| {
            self.fetch_impl(repo, remote, creds, None, Some(on_event), cancel)
        })
    }

    /// 推送指定本地分支（FR-5.2）：无上游时推送同名分支并建立 upstream（等价 `-u`）。
    fn push(
        &self,
        session: &RepoSession,
        branch: &str,
        creds: Option<&Credentials>,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<PushOutcome> {
        self.with_session(session, |_s, repo| {
            let branch = branch.trim();
            if branch.is_empty() {
                return Err(HigitError::new(ErrorCode::Unknown, "no branch to push"));
            }
            repo.find_branch(branch, git2::BranchType::Local)
                .map_err(|_| {
                    HigitError::new(ErrorCode::Unknown, format!("branch not found: {branch}"))
                })?;
            let remote_name = resolve_remote_name(repo, "", Some(branch))?;
            let mut r = repo
                .find_remote(&remote_name)
                .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?;
            let url = r.url().unwrap_or_default().to_string();

            let mut cbs = build_callbacks(creds, &url, progress, cancel);
            // pack/push 进度回调无返回值（无法中止）；取消经 sideband_progress 注入，
            // 打包阶段一旦开始不可中断，取消在下一次操作生效（FR-5.10 的 push 局限）。
            {
                let cancel = cancel.clone();
                cbs.sideband_progress(move |_data| !cancel.is_cancelled());
            }
            cbs.pack_progress(move |stage, done, total| {
                let stage_key = match stage {
                    PackBuilderStage::Deltafication => "Resolving deltas",
                    _ => "Packing objects",
                };
                progress(Progress {
                    stage: stage_key,
                    done: done as u64,
                    total: total as u64,
                });
            });
            cbs.push_transfer_progress(move |done, total, _bytes| {
                progress(Progress {
                    stage: "Pushing objects",
                    done: done as u64,
                    total: total as u64,
                });
            });

            let mut opts = PushOptions::new();
            opts.remote_callbacks(cbs);
            if let Some(px) = self.proxy_opts() {
                opts.proxy_options(px);
            }

            if cancel.is_cancelled() {
                return Err(HigitError::cancelled());
            }
            let had_upstream = repo
                .find_branch(branch, git2::BranchType::Local)
                .ok()
                .and_then(|b| b.upstream().ok())
                .is_some();
            let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
            r.push(std::slice::from_ref(&refspec), Some(&mut opts))
                .map_err(|e| map_net_err(e, &url, cancel.is_cancelled(), creds.is_some()))?;

            if !had_upstream {
                let mut local = repo
                    .find_branch(branch, git2::BranchType::Local)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                local
                    .set_upstream(Some(&format!("{remote_name}/{branch}")))
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            }
            Ok(PushOutcome {
                created_upstream: !had_upstream,
            })
        })
    }

    /// 多分支推送（推送对话框）：refspecs 显式给定，支持 force、安全强制（--force-with-lease）、
    /// 推送标签（--tags）、指定目标远端名或任意 URL（匿名远端）；
    /// git 命令回显 / 远端 sideband 输出 / 逐分支推送结果经 `on_event` 上报。
    fn push_multi(
        &self,
        session: &RepoSession,
        req: &PushRequest,
        creds: Option<&Credentials>,
        on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        cancel: &CancelToken,
    ) -> Result<PushOutcome> {
        self.with_session(session, |_s, repo| {
            let log = |level: &str, text: String| {
                on_event(SyncEvent::Log {
                    level: level.to_string(),
                    text,
                });
            };

            // 组装 refspec（空远端分支 = 同名）；force / 安全强制等价前缀 "+"
            let mut pairs = Vec::with_capacity(req.refspecs.len());
            for (local, remote) in &req.refspecs {
                let local = local.trim();
                if local.is_empty() {
                    continue;
                }
                let remote = if remote.trim().is_empty() {
                    local
                } else {
                    remote.trim()
                };
                pairs.push((local.to_string(), remote.to_string()));
            }
            if pairs.is_empty() {
                return Err(HigitError::new(ErrorCode::Unknown, "no branch to push"));
            }
            let mut refspecs = Vec::with_capacity(pairs.len());
            for (local, remote) in &pairs {
                let prefix = if req.force || req.force_with_lease { "+" } else { "" };
                refspecs.push(format!("{prefix}refs/heads/{local}:refs/heads/{remote}"));
            }

            // 目标远端：任意 URL（匿名远端）> 显式远端名 > 自动解析；
            // 标签引用须在创建远端对象前收集（Remote 与引用迭代都作用在 repo 上）
            let url_mode = !req.remote_url.trim().is_empty();
            if url_mode && req.force_with_lease {
                return Err(HigitError::new(
                    ErrorCode::Unknown,
                    "安全强制推送需要远端跟踪基准，任意 URL 推送不支持（请改用强制推送）",
                ));
            }
            let mut tag_names: Vec<String> = Vec::new();
            if req.tags {
                let glob = repo
                    .references_glob("refs/tags/*")
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                for reference in glob {
                    let reference =
                        reference.map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    if let Ok(name) = reference.name() {
                        // references_glob 返回全名（refs/tags/v1），refspec 用短名
                        tag_names.push(name.trim_start_matches("refs/tags/").to_string());
                    }
                }
                for name in &tag_names {
                    refspecs.push(format!("refs/tags/{name}:refs/tags/{name}"));
                }
            }

            let remote_name = if url_mode {
                String::new()
            } else if !req.remote.trim().is_empty() {
                let name = req.remote.trim();
                repo.find_remote(name).map_err(|_| {
                    HigitError::new(ErrorCode::Unknown, format!("remote not found: {name}"))
                })?;
                name.to_string()
            } else {
                resolve_remote_name(repo, "", None)?
            };
            let mut r = if url_mode {
                repo.remote_anonymous(req.remote_url.trim())
                    .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?
            } else {
                repo.find_remote(&remote_name)
                    .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?
            };
            let url = r.url().unwrap_or_default().to_string();
            let auth = AuthLog::new(on_event);

            if cancel.is_cancelled() {
                return Err(HigitError::cancelled());
            }

            // git 命令回显（TortoiseGit 风格首行）
            let display = pairs
                .iter()
                .map(|(l, rm)| format!("{l}:{rm}"))
                .collect::<Vec<_>>()
                .join(" ");
            let mut flags = String::new();
            if req.force {
                flags.push_str(" --force");
            }
            if req.force_with_lease {
                flags.push_str(" --force-with-lease");
            }
            if req.tags {
                flags.push_str(" --tags");
            }
            let dest = if url_mode {
                url.clone()
            } else {
                remote_name.clone()
            };
            log(
                "dim",
                format!("git push{flags} --progress \"{dest}\" {display}"),
            );
            log("dim", format!("远端: {url}"));
            if req.tags && !tag_names.is_empty() {
                log(
                    "info",
                    format!("同时推送标签 {} 个: {}", tag_names.len(), tag_names.join(" ")),
                );
            }

            // 安全强制（--force-with-lease）：推送前校验远端各分支仍与本地跟踪引用一致；
            // libgit2 推送无内建 lease，预检通过后仍以 force refspec 推送
            if req.force_with_lease {
                let mut cbs_lease = git2::RemoteCallbacks::new();
                apply_credentials(
                    &mut cbs_lease,
                    creds.cloned().unwrap_or(Credentials::SshKey {
                        username: String::new(),
                        key_path: String::new(),
                        passphrase: String::new(),
                        from_store: false,
                    }),
                    Some(&auth),
                );
                set_hostkey_callback(&mut cbs_lease, &url);
                {
                    let conn = r
                        .connect_auth(git2::Direction::Push, Some(cbs_lease), self.proxy_opts())
                        .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?;
                    let mut advertised = std::collections::HashMap::new();
                    for head in conn
                        .list()
                        .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?
                    {
                        advertised.insert(head.name().to_string(), head.oid());
                    }
                    for (_, remote) in &pairs {
                        let remote_ref = format!("refs/heads/{remote}");
                        let Some(&actual) = advertised.get(&remote_ref) else {
                            continue; // 远端不存在该分支：新建推送，无需校验
                        };
                        let expected = repo
                            .find_reference(&format!("refs/remotes/{remote_name}/{remote}"))
                            .ok()
                            .and_then(|x| x.target());
                        if expected != Some(actual) {
                            return Err(HigitError::new(
                                ErrorCode::PushRejected,
                                format!(
                                    "安全强制推送被拒：远端分支 '{remote}' 与本地跟踪引用不一致（远端可能已被他人更新）。请先抓取，或改用强制推送。"
                                ),
                            ));
                        }
                    }
                }
                log("info", "安全强制校验通过：远端与本地跟踪引用一致".to_string());
            }

            let mut cbs = git2::RemoteCallbacks::new();
            // 远端 sideband 输出（remote: ...）按行缓冲上报；返回 false 以支持中止
            {
                let mut buf = String::new();
                let cancel = cancel.clone();
                cbs.sideband_progress(move |data| {
                    if cancel.is_cancelled() {
                        return false;
                    }
                    buf.push_str(&String::from_utf8_lossy(data));
                    while let Some(idx) = buf.find('\n') {
                        let line: String = buf.drain(..=idx).collect();
                        let line = line.trim_end_matches(['\n', '\r']);
                        if !line.is_empty() {
                            log("remote", line.to_string());
                        }
                    }
                    true
                });
            }
            cbs.pack_progress(move |stage, done, total| {
                let stage_key = match stage {
                    PackBuilderStage::Deltafication => "Resolving deltas",
                    _ => "Packing objects",
                };
                on_event(SyncEvent::Progress {
                    stage: stage_key,
                    done: done as u64,
                    total: total as u64,
                });
            });
            cbs.push_transfer_progress(move |done, total, _bytes| {
                on_event(SyncEvent::Progress {
                    stage: "Pushing objects",
                    done: done as u64,
                    total: total as u64,
                });
            });
            cbs.push_update_reference(move |refname, status| match status {
                None => {
                    let short = refname
                        .trim_start_matches("refs/heads/")
                        .trim_start_matches("refs/tags/");
                    log("ok", format!("{short} -> {short}  ✓"));
                    Ok(())
                }
                Some(msg) => {
                    let short = refname
                        .trim_start_matches("refs/heads/")
                        .trim_start_matches("refs/tags/");
                    log("error", format!("{short} -> {short}  ✗ {msg}"));
                    Ok(())
                }
            });
            apply_credentials(
                &mut cbs,
                creds.cloned().unwrap_or(Credentials::SshKey {
                    username: String::new(),
                    key_path: String::new(),
                    passphrase: String::new(),
                    from_store: false,
                }),
                Some(&auth),
            );
            set_hostkey_callback(&mut cbs, &url);

            let mut opts = PushOptions::new();
            opts.remote_callbacks(cbs);
            if let Some(px) = self.proxy_opts() {
                opts.proxy_options(px);
            }

            r.push(&refspecs, Some(&mut opts))
                .map_err(|e| map_net_err(e, &url, cancel.is_cancelled(), creds.is_some()))?;
            auth.succeed();

            // 逐分支建立缺失的上游（等价 -u；推送多分支时目标为各自的远端分支名）；
            // 任意 URL 推送没有可引用的远端名，无法建立跟踪
            let mut created_upstream = false;
            if req.set_upstream && !url_mode {
                for (local, remote) in &pairs {
                    if let Ok(mut b) = repo.find_branch(local, BranchType::Local) {
                        if b.upstream().is_err()
                            && b.set_upstream(Some(&format!("{remote_name}/{remote}")))
                                .is_ok()
                        {
                            created_upstream = true;
                            log(
                                "info",
                                format!("branch '{local}' set up to track '{remote_name}/{remote}'"),
                            );
                        }
                    }
                }
            }
            Ok(PushOutcome { created_upstream })
        })
    }

    fn sync_remote(&self, session: &RepoSession) -> Result<Option<(String, String)>> {
        self.with_session(session, |_s, repo| {
            let Ok(name) = resolve_remote_name(repo, "", None) else {
                return Ok(None);
            };
            let url = repo
                .find_remote(&name)
                .ok()
                .and_then(|r| r.url().ok().map(str::to_string))
                .unwrap_or_default();
            Ok(Some((name, url)))
        })
    }

    fn remotes(&self, session: &RepoSession) -> Result<Vec<(String, String)>> {
        self.with_session(session, |_s, repo| {
            let names = repo
                .remotes()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let mut out = Vec::with_capacity(names.len());
            for i in 0..names.len() {
                if let Ok(Some(name)) = names.get(i) {
                    let url = repo
                        .find_remote(name)
                        .ok()
                        .and_then(|r| r.url().ok().map(str::to_string))
                        .unwrap_or_default();
                    out.push((name.to_string(), url));
                }
            }
            Ok(out)
        })
    }

    fn push_preview(
        &self,
        session: &RepoSession,
        local: &str,
        remote_branch: &str,
    ) -> Result<PushPreview> {
        self.with_session(session, |_s, repo| {
            let branch = repo.find_branch(local, BranchType::Local).map_err(|_| {
                HigitError::new(ErrorCode::Unknown, format!("branch not found: {local}"))
            })?;
            let Some(local_tip) = branch.get().target() else {
                return Ok(PushPreview::default());
            };
            let mut out = PushPreview::default();
            let mut revwalk = repo
                .revwalk()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            revwalk
                .push(local_tip)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            // 远端分支存在则隐藏其历史，剩余即"将上传"的提交；不存在（新建分支）= 全部历史
            if !remote_branch.trim().is_empty() {
                if let Ok(remote_name) = resolve_remote_name(repo, "", Some(local)) {
                    let remote_ref = format!("refs/remotes/{remote_name}/{}", remote_branch.trim());
                    if let Ok(r) = repo.find_reference(&remote_ref) {
                        if let Some(tip) = r.target() {
                            let _ = revwalk.hide(tip);
                        }
                    }
                }
            }
            let mut files = std::collections::BTreeSet::new();
            for oid in revwalk.flatten() {
                out.commits = out.commits.saturating_add(1);
                if out.commits >= 1000 {
                    break; // 预览上限：避免超大仓库遍历过久
                }
                let Ok(commit) = repo.find_commit(oid) else {
                    continue;
                };
                let Ok(tree) = commit.tree() else {
                    continue;
                };
                let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
                let Ok(diff) = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
                else {
                    continue;
                };
                for d in diff.deltas() {
                    if let Some(p) = d.new_file().path().or_else(|| d.old_file().path()) {
                        files.insert(p.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
            out.files = files.into_iter().collect();
            Ok(out)
        })
    }

    /// 拉取（FR-5.1）：fetch 指定分支的上游 + 快进合并该分支。
    /// 分叉（双方均有新提交）返回 `Diverged`；安全模式下若与未提交修改冲突则拒绝覆盖。
    /// 分支非当前检出时仅前移本地引用，不更新工作区。默认选项 = 既有行为。
    fn pull(
        &self,
        session: &RepoSession,
        branch: &str,
        creds: Option<&Credentials>,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        self.pull_options(
            session,
            branch,
            &PullOptions::default(),
            creds,
            Some(progress),
            None,
            cancel,
        )
    }

    /// 拉取（拉取对话框）：同 [`GitEngine::pull`]，过程经 `on_event` 详细上报。
    fn pull_verbose(
        &self,
        session: &RepoSession,
        branch: &str,
        creds: Option<&Credentials>,
        on_event: &(dyn Fn(SyncEvent) + Send + Sync),
        cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        self.pull_options(
            session,
            branch,
            &PullOptions::default(),
            creds,
            None,
            Some(on_event),
            cancel,
        )
    }

    /// 增强拉取（FR-5.1 扩展）：抓取（--prune / --tags）后按选项整合——
    /// 快进 / 合并（--no-ff / --no-commit / --squash）或变基（--rebase）。
    fn pull_options(
        &self,
        session: &RepoSession,
        branch: &str,
        opts: &PullOptions,
        creds: Option<&Credentials>,
        progress: Option<&dyn Fn(Progress)>,
        on_event: Option<&(dyn Fn(SyncEvent) + Send + Sync)>,
        cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        if opts.no_ff && opts.ff_only {
            return Err(HigitError::new(
                ErrorCode::Unknown,
                "cannot combine --no-ff with --ff-only",
            ));
        }
        if opts.rebase && opts.squash {
            return Err(HigitError::new(
                ErrorCode::Unknown,
                "cannot combine --rebase with --squash",
            ));
        }
        let plan = self.with_session(session, |_s, repo| {
            self.pull_fetch_plan(repo, branch, opts, creds, progress, on_event, cancel)
        })?;
        match plan {
            PullPlan::UpToDate => Ok(PullOutcome::default()),
            PullPlan::FastForward { up_oid, up_name } => self.with_session(session, |_s, repo| {
                self.pull_fast_forward(repo, branch, up_oid, &up_name, &sync_logger(on_event))
            }),
            PullPlan::Rebase { up_name, up_oid } => self.with_session(session, |_s, repo| {
                self.pull_rebase(repo, branch, &up_name, up_oid, &sync_logger(on_event))
            }),
            PullPlan::Merge { up_name } => {
                // 分叉 / no_ff / squash：复用增强合并的全部选项语义
                let o = self.merge(
                    session,
                    &MergeOptions {
                        kind: CheckoutTargetKind::Branch,
                        source: up_name,
                        no_ff: opts.no_ff,
                        ff_only: false,
                        no_commit: opts.no_commit,
                        squash: opts.squash,
                        ..Default::default()
                    },
                )?;
                let log = sync_logger(on_event);
                if !o.conflicts.is_empty() {
                    log(
                        "warn",
                        format!(
                            "merge blocked by conflicts in {} file(s); resolve them and commit",
                            o.conflicts.len()
                        ),
                    );
                    return Ok(PullOutcome {
                        conflicts: o.conflicts,
                        ..Default::default()
                    });
                }
                if o.pending_commit {
                    log(
                        "ok",
                        "merge prepared without commit (--no-commit / --squash); commit manually".into(),
                    );
                    return Ok(PullOutcome {
                        pending_commit: true,
                        ..Default::default()
                    });
                }
                let head = self.with_session(session, |_s, repo| head_info_of(repo))?;
                log(
                    "ok",
                    format!(
                        "merged as commit {}",
                        o.created_commit.clone().unwrap_or_default()
                    ),
                );
                Ok(PullOutcome {
                    updated: o.created_commit.is_some(),
                    head: Some(head),
                    fast_forward: false,
                    rebased: 0,
                    pending_commit: false,
                    conflicts: Vec::new(),
                })
            }
        }
    }

    fn set_proxy(&self, proxy: Option<&str>) {
        let url = proxy
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        *self.proxy.lock().expect("proxy mutex poisoned") = url;
    }

    fn stage(&self, session: &RepoSession, paths: &[PathBuf]) -> Result<()> {
        self.with_session(session, |s, repo| {
            let mut index = repo
                .index()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            // 暂存单个路径：文件仍在工作区则 add；已删除则暂存删除
            // （add_bypath 不处理删除，等价 `git add` 对删除项的语义）；索引里也没有则忽略
            let stage_one = |index: &mut git2::Index, p: &str| -> Result<()> {
                if s.workdir.join(p).exists() {
                    index
                        .add_path(Path::new(p))
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))
                } else {
                    match index.remove_path(Path::new(p)) {
                        Ok(()) => Ok(()),
                        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
                        Err(e) => Err(HigitError::new(ErrorCode::Unknown, e.message())),
                    }
                }
            };
            for path in paths {
                let rel = path.strip_prefix(&s.workdir).unwrap_or(path);
                // libgit2 要求相对路径使用 “/” 分隔
                let rel = PathBuf::from(rel.to_string_lossy().replace('\\', "/"));
                let abs = s.workdir.join(&rel);
                if abs.is_dir() {
                    // 目录：展开为其下全部有变更的文件（跳过忽略项，FR-3.2 目录语义）
                    let prefix = {
                        let mut p = rel.to_string_lossy().to_string();
                        if !p.ends_with('/') {
                            p.push('/');
                        }
                        p
                    };
                    let mut opts = git2::StatusOptions::new();
                    opts.include_untracked(true).include_ignored(false);
                    opts.pathspec(prefix.clone());
                    let statuses = repo
                        .statuses(Some(&mut opts))
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    for item in statuses.iter() {
                        let Ok(p) = item.path() else { continue };
                        if !p.starts_with(&prefix) {
                            continue;
                        }
                        let st = item.status();
                        if st.is_empty() || st.intersects(Status::IGNORED) {
                            continue;
                        }
                        stage_one(&mut index, p)?;
                    }
                } else {
                    stage_one(&mut index, &rel.to_string_lossy())?;
                }
            }
            index
                .write()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 取消暂存（FR-3.3）：把索引条目重置回 HEAD（等价 `git reset`）。
    fn unstage(&self, session: &RepoSession, paths: &[PathBuf]) -> Result<()> {
        self.with_session(session, |s, repo| {
            for path in paths {
                let rel = rel_of(s, path);
                repo.reset_default(None, [rel.to_string_lossy().to_string()])
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            }
            Ok(())
        })
    }

    /// 丢弃更改（FR-3.4）：已跟踪文件强制 checkout HEAD；未跟踪文件直接删除。
    fn discard(&self, session: &RepoSession, paths: &[PathBuf]) -> Result<()> {
        self.with_session(session, |s, repo| {
            let mut tracked = Vec::new();
            for path in paths {
                let rel = rel_of(s, path);
                let status = repo
                    .status_file(&rel)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                if status.intersects(Status::WT_NEW) {
                    // 未跟踪：从磁盘删除（文件或整个未跟踪目录）
                    let abs = s.workdir.join(&rel);
                    let meta = std::fs::metadata(&abs);
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let result = if is_dir {
                        std::fs::remove_dir_all(&abs)
                    } else {
                        std::fs::remove_file(&abs)
                    };
                    result.map_err(|e| HigitError::new(ErrorCode::Unknown, e.to_string()))?;
                } else {
                    tracked.push(rel);
                }
            }
            if !tracked.is_empty() {
                let mut cb = git2::build::CheckoutBuilder::new();
                cb.force();
                for rel in &tracked {
                    cb.path(rel.to_string_lossy().to_string());
                }
                repo.checkout_head(Some(&mut cb))
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            }
            Ok(())
        })
    }

    /// 工作区清理（git clean 语义）：按状态列举未跟踪 / 被忽略条目后删除。
    /// 不递归列举 → 未跟踪 / 被忽略目录聚合为 `dir/` 条目，与 -d 语义一致（不勾选则整体跳过）；
    /// 含 .git 的目录视为嵌套仓库，force_git_dirs（-ff）为 false 时跳过；
    /// 默认移入回收站，no_recycle_bin = true 时直接删除；子模块条目始终排除。
    fn clean_worktree(&self, session: &RepoSession, opts: &CleanOptions) -> Result<CleanOutcome> {
        self.with_session(session, |s, repo| {
            let mut sopts = git2::StatusOptions::new();
            sopts.include_untracked(true).include_ignored(true).exclude_submodules(true);
            let statuses = repo
                .statuses(Some(&mut sopts))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;

            // 筛选目标条目：(相对路径, 是否目录条目)。git2 目录条目路径带尾斜杠
            let mut targets: Vec<(PathBuf, bool)> = Vec::new();
            for entry in statuses.iter() {
                let st = entry.status();
                let ignored = st.is_ignored();
                let untracked = st.is_wt_new();
                if !ignored && !untracked {
                    continue;
                }
                let want = if ignored {
                    matches!(opts.mode, CleanMode::All | CleanMode::IgnoredOnly)
                } else {
                    matches!(opts.mode, CleanMode::All | CleanMode::UntrackedOnly)
                };
                if !want {
                    continue;
                }
                let raw = match entry.path() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let is_dir = raw.ends_with('/');
                if is_dir && !opts.dirs {
                    continue;
                }
                let rel = PathBuf::from(raw.replace('\\', "/"));
                // 含 .git 的目录 = 嵌套仓库：git clean -fd 拒绝删除，需 -ff 才移除
                if is_dir && !opts.force_git_dirs && s.workdir.join(&rel).join(".git").exists() {
                    continue;
                }
                targets.push((rel, is_dir));
            }

            let mut outcome = CleanOutcome::default();
            for (rel, is_dir) in &targets {
                if opts.dry_run {
                    outcome.paths.push(rel.clone());
                    continue;
                }
                let abs = s.workdir.join(rel);
                let result = if opts.no_recycle_bin {
                    if *is_dir {
                        std::fs::remove_dir_all(&abs)
                    } else {
                        std::fs::remove_file(&abs)
                    }
                } else {
                    trash::delete(&abs).map_err(|e| std::io::Error::other(e.to_string()))
                };
                match result {
                    Ok(()) => outcome.paths.push(rel.clone()),
                    Err(e) => outcome
                        .failed
                        .push((rel.clone(), e.to_string())),
                }
            }
            Ok(outcome)
        })
    }

    fn commit(
        &self,
        session: &RepoSession,
        message: &str,
        author: Option<&Identity>,
    ) -> Result<String> {
        self.with_session(session, |_s, repo| {
            let identity = resolve_identity(repo, author)?;
            let sig = Signature::now(&identity.name, &identity.email)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            let tree = index_tree(repo)?;

            let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
            let mut parents: Vec<git2::Commit> = parent.into_iter().collect();
            // MERGE_HEAD 存在 = 冲突已解决的合并收尾：产出双亲合并提交并清理合并状态（FR-5.8）
            let merge_head = repo.find_reference("MERGE_HEAD").ok();
            if let Some(mh) = &merge_head {
                if let Ok(p) = mh.peel_to_commit() {
                    parents.push(p);
                }
            }
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            let oid = repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            if merge_head.is_some() {
                let _ = repo.cleanup_state();
            }
            Ok(oid.to_string())
        })
    }

    /// 修补上一次提交（FR-4.5）：用当前暂存区与给定消息替换 HEAD 提交；
    /// 作者未显式给出时保留原提交作者（等价 `git commit --amend` 语义）。
    fn commit_amend(
        &self,
        session: &RepoSession,
        message: &str,
        author: Option<&Identity>,
    ) -> Result<String> {
        self.with_session(session, |_s, repo| {
            let head = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            // 作者未显式给出时保留原提交作者；原提交缺身份再回退配置
            let identity = match author {
                Some(id) => id.clone(),
                None => {
                    let a = head.author();
                    let name = a.name().unwrap_or_default().to_string();
                    let email = a.email().unwrap_or_default().to_string();
                    if name.is_empty() || email.is_empty() {
                        resolve_identity(repo, None)?
                    } else {
                        Identity { name, email }
                    }
                }
            };
            let sig = Signature::now(&identity.name, &identity.email)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let tree = index_tree(repo)?;

            let parents: Vec<git2::Commit> = head.parents().collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            // libgit2 对 update_ref 校验“首父 = 引用当前 tip”，amend 首父是原提交的父，
            // 因此先建提交对象（不更新引用）再手动前移 HEAD 指向的分支引用。
            let oid = repo
                .commit(None, &sig, &sig, message, &tree, &parent_refs)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let mut head_ref = repo
                .head()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            head_ref
                .set_target(oid, "commit: amend")
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(oid.to_string())
        })
    }

    /// 提交预览（提交对话框）：工作区全部变更文件（含未暂存 / 未跟踪 / 冲突），
    /// 仅列 `dir` 目录下的条目；细化状态 + 是否已暂存 + 磁盘大小 / 修改时间；
    /// 文件不存在一律按删除展示。
    fn commit_preview(&self, session: &RepoSession, dir: &str) -> Result<Vec<ChangeFileInfo>> {
        self.with_session(session, |s, repo| {
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true).recurse_untracked_dirs(true);
            let statuses = repo
                .statuses(Some(&mut opts))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;

            let dir = dir.trim_matches('/');
            let dir_prefix = if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            };
            let mut files = Vec::new();
            for item in statuses.iter() {
                let st = item.status();
                let Ok(path) = item.path() else {
                    continue;
                };
                let path = path.trim_end_matches('/');
                if st.is_empty() || st.intersects(git2::Status::IGNORED) {
                    continue;
                }
                if !dir_prefix.is_empty() && path != dir && !path.starts_with(&dir_prefix) {
                    continue;
                }
                let md = std::fs::metadata(s.workdir.join(path)).ok();
                let staged = st.intersects(
                    git2::Status::INDEX_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_DELETED
                        | git2::Status::INDEX_RENAMED
                        | git2::Status::INDEX_TYPECHANGE,
                );
                let kind = if st.contains(git2::Status::CONFLICTED) {
                    "conflicted"
                } else if md.is_none() {
                    // 磁盘上不存在：暂存删除 / 未暂存删除 / 暂存新增后又被删，都按删除展示
                    "deleted"
                } else if st.intersects(git2::Status::WT_NEW | git2::Status::INDEX_NEW) {
                    "added"
                } else if st.intersects(git2::Status::INDEX_RENAMED | git2::Status::WT_RENAMED) {
                    "renamed"
                } else {
                    "modified"
                };
                let size = md.as_ref().map(|m| m.len());
                let mtime_unix = md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                files.push(ChangeFileInfo {
                    path: path.to_string(),
                    kind: kind.to_string(),
                    staged,
                    size,
                    mtime_unix,
                    additions: None,
                    deletions: None,
                });
            }
            Ok(files)
        })
    }

    /// 提交对话框的行级变更统计（+行 / -行）：逐文件 diff HEAD↔工作区（含索引）。
    /// 每个文件一次 pathspec 限定 diff；文件数超过上限时整体跳过（性能保护）。
    fn change_stats(&self, session: &RepoSession, paths: &[String]) -> Result<Vec<(Option<u32>, Option<u32>)>> {
        self.with_session(session, |_s, repo| {
            if paths.len() > COMMIT_STATS_MAX_FILES {
                return Ok(vec![(None, None); paths.len()]);
            }
            let head_tree = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .and_then(|c| c.tree().ok());
            let mut out = Vec::with_capacity(paths.len());
            for p in paths {
                let mut dopts = git2::DiffOptions::new();
                // 未跟踪文件需要 SHOW_UNTRACKED_CONTENT 才会统计内容行数
                dopts
                    .pathspec(p)
                    .include_untracked(true)
                    .show_untracked_content(true);
                let stats = repo
                    .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut dopts))
                    .ok()
                    .and_then(|d| d.stats().ok());
                out.push(match stats {
                    Some(st) => (Some(st.insertions() as u32), Some(st.deletions() as u32)),
                    None => (None, None),
                });
            }
            Ok(out)
        })
    }

    /// 按路径提交（git commit -- <paths> 语义）：勾选路径先按工作区内容暂存进真实索引
    /// （未暂存修改 / 未跟踪新增 / 已删除与 stage 同语义一并纳入），临时索引 = HEAD 树 +
    /// 这些路径的最新暂存条目；范围外的暂存内容不进入提交且保持暂存状态。
    fn commit_partial(
        &self,
        session: &RepoSession,
        message: &str,
        author: Option<&Identity>,
        paths: &[String],
    ) -> Result<String> {
        self.with_session(session, |s, repo| {
            let identity = resolve_identity(repo, author)?;
            let sig = Signature::now(&identity.name, &identity.email)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            partial_commit_impl(repo, &s.workdir, message, &sig, paths, false)
        })
    }

    /// 按路径修补上一次提交（提交对话框 Amend + 勾选语义）：树 = HEAD 树 + 路径最新
    /// 暂存内容，父提交沿用原提交（合并提交修补后仍为合并提交）；仅改消息也合法。
    fn commit_amend_partial(
        &self,
        session: &RepoSession,
        message: &str,
        author: Option<&Identity>,
        paths: &[String],
    ) -> Result<String> {
        self.with_session(session, |s, repo| {
            let identity = resolve_identity(repo, author)?;
            let sig = Signature::now(&identity.name, &identity.email)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            partial_commit_impl(repo, &s.workdir, message, &sig, paths, true)
        })
    }

    /// 仅移动 HEAD 符号引用到指定本地分支（不更新工作区 / 索引）。
    /// "提交到新分支"场景下新分支与 HEAD 同树，无需检出任何文件。
    fn switch_head(&self, session: &RepoSession, branch: &str) -> Result<()> {
        self.with_session(session, |_s, repo| {
            // 先校验分支存在，给出明确错误而非 set_head 的模糊失败
            repo.find_branch(branch, BranchType::Local)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            repo.set_head(&format!("refs/heads/{branch}"))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 单文件文本差异（FR-8.1/8.4）。
    fn diff_text(&self, session: &RepoSession, path: &Path, spec: DiffSpec) -> Result<TextDiff> {
        self.with_session(session, |s, repo| {
            let rel = rel_of(s, path);
            let abs = s.workdir.join(&rel);

            // >1MB 文本只返回统计标志（FR-8.4）
            if let Ok(meta) = std::fs::metadata(&abs) {
                if meta.is_file() && meta.len() > DIFF_TEXT_MAX_BYTES {
                    return Ok(TextDiff {
                        too_large: true,
                        ..Default::default()
                    });
                }
            }

            let mut opts = git2::DiffOptions::new();
            opts.pathspec(rel.to_string_lossy().to_string());
            opts.include_untracked(true);
            opts.recurse_untracked_dirs(true);
            opts.show_untracked_content(true);

            let diff = match spec {
                DiffSpec::Workdir => repo
                    .diff_index_to_workdir(None, Some(&mut opts))
                    .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?,
                DiffSpec::Staged => {
                    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                    repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
                        .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?
                }
            };

            extract_diff_lines(&diff)
        })
    }

    /// 提交历史（FR-6.1）：HEAD 按时间倒序 + 引用装饰；skip 用于滚动分页。
    fn log(&self, session: &RepoSession, limit: u32, skip: u32) -> Result<Vec<CommitEntry>> {
        self.with_session(session, |_s, repo| {
            // 空仓库（HEAD 未出生）无历史
            let Ok(head_ref) = repo.head() else {
                return Ok(Vec::new());
            };
            let head_oid = head_ref.target();
            let head_branch = current_branch(repo);

            let mut walk = repo
                .revwalk()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            walk.push_head()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            walk.set_sorting(Sort::TIME)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            // 引用装饰：本地/远端分支名按目标提交聚合
            let mut refs_by_oid: HashMap<Oid, Vec<String>> = HashMap::new();
            if let Ok(refs) = repo.references() {
                for r in refs.flatten() {
                    let full = r.name().unwrap_or_default();
                    let Some(short) = full
                        .strip_prefix("refs/heads/")
                        .or_else(|| full.strip_prefix("refs/remotes/"))
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    if let Some(target) = r.target() {
                        refs_by_oid.entry(target).or_default().push(short);
                    }
                }
            }
            for names in refs_by_oid.values_mut() {
                names.sort();
            }

            let mut out = Vec::with_capacity(limit as usize);
            for oid in walk.skip(skip as usize).take(limit as usize) {
                let Ok(oid) = oid else { continue };
                let Ok(commit) = repo.find_commit(oid) else {
                    continue;
                };
                let mut refs = refs_by_oid.get(&oid).cloned().unwrap_or_default();
                if Some(oid) == head_oid {
                    // HEAD 所指分支标注 "HEAD -> name"
                    if let Some(b) = &head_branch {
                        refs = refs
                            .into_iter()
                            .map(|r| if r == *b { format!("HEAD -> {b}") } else { r })
                            .collect();
                    }
                }
                let author = commit.author();
                out.push(CommitEntry {
                    id: oid.to_string(),
                    short_id: short_id(oid),
                    summary: commit
                        .summary()
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                        .to_string(),
                    author_name: author.name().unwrap_or("").to_string(),
                    author_email: author.email().unwrap_or("").to_string(),
                    time_unix: commit.time().seconds(),
                    refs,
                    parents: commit.parent_count(),
                });
            }
            Ok(out)
        })
    }

    /// 提交详情（FR-6.2）：完整消息 + 父提交 + 变更文件（含重命名识别与行数统计）。
    fn commit_detail(&self, session: &RepoSession, oid: &str) -> Result<CommitDetail> {
        self.with_session(session, |_s, repo| {
            let commit = resolve_commit(repo, oid)?;
            let author = commit.author();
            let committer = commit.committer();
            let mut detail = CommitDetail {
                id: commit.id().to_string(),
                short_id: short_id(commit.id()),
                summary: commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_string(),
                message: commit.message().unwrap_or_default().trim_end().to_string(),
                author_name: author.name().unwrap_or("").to_string(),
                author_email: author.email().unwrap_or("").to_string(),
                committer_name: committer.name().unwrap_or("").to_string(),
                committer_email: committer.email().unwrap_or("").to_string(),
                time_unix: commit.time().seconds(),
                parents: commit.parent_ids().map(|p| p.to_string()).collect(),
                files: Vec::new(),
            };

            let tree = commit
                .tree()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let mut opts = git2::DiffOptions::new();
            let mut diff = repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;
            diff.find_similar(Some(git2::DiffFindOptions::new().renames(true)))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;

            collect_commit_files(&mut diff, &mut detail.files)?;
            Ok(detail)
        })
    }

    /// 提交内单文件差异（FR-6.3 / FR-8.2）：该提交 vs 第一父提交（根提交 vs 空树）。
    fn commit_file_diff(&self, session: &RepoSession, oid: &str, path: &Path) -> Result<TextDiff> {
        self.with_session(session, |_s, repo| {
            let commit = resolve_commit(repo, oid)?;
            let tree = commit
                .tree()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

            let mut opts = git2::DiffOptions::new();
            opts.pathspec(normalize_rel(path).to_string_lossy().to_string());
            let diff = repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
                .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;
            extract_diff_lines(&diff)
        })
    }

    /// 分支列表（FR-7.1）：本地在前远端在后；本地分支附上游与 ahead/behind。
    fn branches(&self, session: &RepoSession) -> Result<Vec<BranchInfo>> {
        self.with_session(session, |_s, repo| {
            let mut out = Vec::new();
            let locals = repo
                .branches(Some(BranchType::Local))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            for item in locals.flatten() {
                let (branch, _) = item;
                let Some(name) = branch.name().ok().flatten() else {
                    continue;
                };
                let reference = branch.get();
                let Some(target) = reference.target() else {
                    continue;
                };
                let (upstream, ahead, behind) = match branch.upstream() {
                    Ok(up) => match (up.name().ok().flatten(), up.get().target()) {
                        (Some(uname), Some(utarget)) => {
                            let (a, b) = repo.graph_ahead_behind(target, utarget).unwrap_or((0, 0));
                            (Some(uname.to_string()), a as u32, b as u32)
                        }
                        _ => (None, 0, 0),
                    },
                    Err(_) => (None, 0, 0),
                };
                let summary = reference
                    .peel_to_commit()
                    .ok()
                    .and_then(|c| c.summary().ok().flatten().map(str::to_string))
                    .unwrap_or_default();
                out.push(BranchInfo {
                    name: name.to_string(),
                    is_remote: false,
                    is_current: branch.is_head(),
                    short_id: short_id(target),
                    last_commit_summary: summary,
                    upstream,
                    ahead,
                    behind,
                });
            }
            let remotes = repo
                .branches(Some(BranchType::Remote))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            for item in remotes.flatten() {
                let (branch, _) = item;
                // origin/HEAD 等符号引用只是默认分支的别名（origin/HEAD -> origin/main），
                // 不是可检出的实体分支；检出时会因保留名 "HEAD" 报错，这里直接跳过
                if branch.get().kind() == Some(git2::ReferenceType::Symbolic) {
                    continue;
                }
                if let Some(name) = branch.name().ok().flatten() {
                    let reference = branch.get();
                    let short = reference.target().map(short_id).unwrap_or_default();
                    let summary = reference
                        .peel_to_commit()
                        .ok()
                        .and_then(|c| c.summary().ok().flatten().map(str::to_string))
                        .unwrap_or_default();
                    out.push(BranchInfo {
                        name: name.to_string(),
                        is_remote: true,
                        is_current: false,
                        short_id: short,
                        last_commit_summary: summary,
                        upstream: None,
                        ahead: 0,
                        behind: 0,
                    });
                }
            }
            Ok(out)
        })
    }

    /// 新建分支（FR-7.2）。`from` 为空 = HEAD；否则 revparse 解析（hash / 分支 / 表达式）。
    fn create_branch(&self, session: &RepoSession, name: &str, from: Option<&str>) -> Result<()> {
        self.with_session(session, |_s, repo| {
            let commit = match from.filter(|f| !f.trim().is_empty()) {
                Some(f) => resolve_commit(repo, f.trim())?,
                None => repo.head().and_then(|h| h.peel_to_commit()).map_err(|_| {
                    HigitError::new(
                        ErrorCode::Unknown,
                        "cannot create branch: repository has no commits yet",
                    )
                })?,
            };
            repo.branch(name, &commit, false)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 切换分支（FR-7.1/7.2）：安全检出（不覆盖未提交修改），随后移动 HEAD。
    fn checkout_branch(&self, session: &RepoSession, name: &str) -> Result<()> {
        self.with_session(session, |_s, repo| {
            let branch = repo
                .find_branch(name, BranchType::Local)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let commit = branch
                .get()
                .peel_to_commit()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let mut opts = git2::build::CheckoutBuilder::new(); // 默认 SAFE
            repo.checkout_tree(commit.as_object(), Some(&mut opts))
                .map_err(map_checkout_err)?;
            repo.set_head(&format!("refs/heads/{name}"))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 标签列表（FR-7.5）：轻量与附注标签统一 peel 到提交；按提交时间倒序。
    fn tags(&self, session: &RepoSession) -> Result<Vec<TagInfo>> {
        self.with_session(session, |_s, repo| {
            let mut out: Vec<(i64, TagInfo)> = Vec::new();
            let refs = repo
                .references_glob("refs/tags/*")
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            for item in refs {
                let r = item.map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                let Ok(full) = r.name() else { continue };
                let Some(oid) = r.target() else { continue };
                let name = full.trim_start_matches("refs/tags/").to_string();
                // 附注标签对象与轻量标签（直接指向提交）统一解析到提交
                let Ok(commit) = repo.find_object(oid, None).and_then(|o| o.peel_to_commit())
                else {
                    continue;
                };
                let summary = commit
                    .summary()
                    .ok()
                    .flatten()
                    .map(str::to_string)
                    .unwrap_or_default();
                out.push((
                    commit.time().seconds(),
                    TagInfo {
                        name,
                        short_id: short_id(commit.id()),
                        last_commit_summary: summary,
                    },
                ));
            }
            out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
            Ok(out.into_iter().map(|(_, t)| t).collect())
        })
    }

    /// 增强检出（FR-7.5）：分支（本地/远端）/ 标签 / 提交 + 新建分支、强制、
    /// 合并本地修改（stash→检出→恢复，git checkout -m 语义）、跟踪远端分支、
    /// 覆盖已存在分支（branch -f）。
    ///
    /// git2 的 Commit/Branch 等句柄借用 `&Repository`，而 stash 系列要求 `&mut`，
    /// 因此各阶段在独立作用域内取对象、用完即弃，阶段之间不保留任何借用。
    fn checkout(&self, session: &RepoSession, opts: &CheckoutOptions) -> Result<CheckoutOutcome> {
        self.with_session_mut(session, |_s, repo| {
            let mut outcome = CheckoutOutcome::default();
            let target = opts.target.trim();
            if target.is_empty() {
                return Err(HigitError::new(ErrorCode::Unknown, "checkout target is empty"));
            }

            // 1) 目标解析为提交 id；分支目标额外记录远端引用名（跟踪用）
            let (commit_id, remote_ref): (Oid, Option<String>) = match opts.kind {
                CheckoutTargetKind::Branch => {
                    match repo.find_branch(target, BranchType::Local) {
                        Ok(b) => (
                            b.get().peel_to_commit().map_err(map_checkout_err)?.id(),
                            None,
                        ),
                        // 本地不存在 → 尝试远端分支（如 "origin/p2-1"）
                        Err(_) => {
                            let b = repo
                                .find_branch(target, BranchType::Remote)
                                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                            (
                                b.get().peel_to_commit().map_err(map_checkout_err)?.id(),
                                Some(target.to_string()),
                            )
                        }
                    }
                }
                CheckoutTargetKind::Tag => {
                    let obj = repo
                        .revparse_single(&format!("refs/tags/{target}"))
                        .or_else(|_| repo.revparse_single(target))
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    (
                        obj.peel_to_commit()
                            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?
                            .id(),
                        None,
                    )
                }
                CheckoutTargetKind::Commit => (resolve_commit(repo, target)?.id(), None),
            };

            // 2) merge：先把未提交修改 stash 起来，检出后恢复（工作区无改动时跳过）
            let stashed = if opts.merge { stash_workdir(repo)? } else { false };

            // 3) 确定 HEAD 去向：新建分支 / 跟踪远端 → 先建本地分支；
            //    远端不跟踪或标签 / 提交 → 分离 HEAD；否则指向本地分支
            let head_ref: String;
            {
                let commit = repo
                    .find_commit(commit_id)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                if let Some(new_name) = opts
                    .create_branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let mut branch = repo
                        .branch(new_name, &commit, opts.override_existing)
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    if let Some(up_ref) = remote_ref.as_deref().filter(|_| opts.track) {
                        branch
                            .set_upstream(Some(up_ref))
                            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    }
                    head_ref = format!("refs/heads/{new_name}");
                    outcome.created_branch = Some(new_name.to_string());
                } else if let Some(remote_name) = remote_ref.as_deref() {
                    if opts.track {
                        // git checkout --track origin/x：建同名本地分支并建立跟踪
                        let short = local_branch_name(repo, remote_name)?;
                        let mut branch = repo
                            .branch(&short, &commit, opts.override_existing)
                            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                        branch
                            .set_upstream(Some(remote_name))
                            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                        head_ref = format!("refs/heads/{short}");
                        outcome.created_branch = Some(short);
                    } else {
                        head_ref = String::new();
                    }
                } else if opts.kind == CheckoutTargetKind::Branch {
                    head_ref = format!("refs/heads/{target}");
                } else {
                    head_ref = String::new();
                }
            }

            // 4) 检出工作区并移动 HEAD（默认安全；force 覆盖工作区修改）
            {
                let mut co = git2::build::CheckoutBuilder::new();
                if opts.force {
                    co.force();
                }
                let obj = repo
                    .find_object(commit_id, None)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                repo.checkout_tree(&obj, Some(&mut co))
                    .map_err(map_checkout_err)?;
                if head_ref.is_empty() {
                    repo.set_head_detached(commit_id)
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    outcome.detached = true;
                } else {
                    repo.set_head(&head_ref)
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                }
            }

            // 5) merge：恢复 stash 的修改；恢复冲突时保留冲突状态待解决
            if stashed {
                match repo.stash_pop(0, None) {
                    Ok(()) => {
                        // libgit2 stash apply 冲突时不报错：冲突条目与标记已写入
                        // 索引 / 工作区，与 git stash pop 冲突的结果一致
                        if repo.index().map(|i| i.has_conflicts()).unwrap_or(false) {
                            outcome.conflicted = true;
                        }
                    }
                    Err(e) => return Err(HigitError::new(ErrorCode::Unknown, e.message())),
                }
            }

            Ok(outcome)
        })
    }

    /// 重命名分支（FR-7.3）；当前分支由 libgit2 自动跟随 HEAD。
    fn rename_branch(&self, session: &RepoSession, old: &str, new: &str) -> Result<()> {
        self.with_session(session, |_s, repo| {
            let mut branch = repo
                .find_branch(old, BranchType::Local)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            branch
                .rename(new, false)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 删除分支（FR-7.3）：未合并且 !force 时返回 `BranchNotMerged`（UI 强确认后重试）。
    fn delete_branch(&self, session: &RepoSession, name: &str, force: bool) -> Result<()> {
        self.with_session(session, |_s, repo| {
            if current_branch(repo).as_deref() == Some(name) {
                return Err(HigitError::new(
                    ErrorCode::Unknown,
                    "cannot delete the currently checked out branch",
                ));
            }
            let mut branch = repo
                .find_branch(name, BranchType::Local)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let Some(target) = branch.get().target() else {
                return Err(HigitError::new(ErrorCode::Unknown, "branch has no target"));
            };
            let merged = match repo.head().and_then(|h| h.peel_to_commit()) {
                Ok(head) => {
                    head.id() == target
                        || repo.graph_descendant_of(head.id(), target).unwrap_or(false)
                }
                Err(_) => false,
            };
            if !merged && !force {
                return Err(HigitError::new(ErrorCode::BranchNotMerged, name));
            }
            branch
                .delete()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 合并到当前分支（FR-7.4）：优先快进，否则生成合并提交；
    /// 冲突时阻断并列出冲突文件（FR-5.8 P0），仓库保留冲突状态待解决后提交。
    /// 简化入口，委托给增强合并 `merge`（默认选项）。
    fn merge_branch(&self, session: &RepoSession, name: &str) -> Result<MergeOutcome> {
        let r = self.merge(
            session,
            &MergeOptions {
                kind: CheckoutTargetKind::Branch,
                source: name.to_string(),
                ..Default::default()
            },
        )?;
        Ok(MergeOutcome {
            fast_forward: r.fast_forward,
            updated: r.updated,
            conflicts: r.conflicts,
        })
    }

    /// 增强合并（FR-7.6）：来源支持分支（本地/远端）/ 标签 / 提交，附 TortoiseGit 式
    /// 选项：--no-ff / --ff-only / --no-commit / --squash、自定义提交消息、
    /// 附加日志消息（merge.log）、-s ours 策略。
    fn merge(&self, session: &RepoSession, opts: &MergeOptions) -> Result<MergeResult> {
        self.with_session(session, |_s, repo| {
            let mut out = MergeResult::default();
            let source = opts.source.trim();
            if source.is_empty() {
                return Err(HigitError::new(ErrorCode::Unknown, "merge source is empty"));
            }
            if opts.no_ff && opts.ff_only {
                return Err(HigitError::new(
                    ErrorCode::Unknown,
                    "cannot combine --no-ff with --ff-only",
                ));
            }

            // 工作区必须干净（除未跟踪），避免合并中途覆盖用户修改
            {
                let mut sopts = git2::StatusOptions::new();
                sopts.include_untracked(false);
                let statuses = repo
                    .statuses(Some(&mut sopts))
                    .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;
                if statuses.iter().any(|i| !i.status().is_empty()) {
                    return Err(HigitError::new(
                        ErrorCode::DirtyWorktree,
                        "merge requires a clean working tree",
                    ));
                }
            }

            // 来源解析为提交 + 消息/日志用的描述与短标签
            let (their_commit, source_desc) = match opts.kind {
                CheckoutTargetKind::Branch => {
                    match repo.find_branch(source, BranchType::Local) {
                        Ok(b) => (
                            b.get()
                                .peel_to_commit()
                                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?,
                            format!("branch '{source}'"),
                        ),
                        Err(_) => {
                            let b = repo
                                .find_branch(source, BranchType::Remote)
                                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                            (
                                b.get()
                                    .peel_to_commit()
                                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?,
                                format!("remote-tracking branch '{source}'"),
                            )
                        }
                    }
                }
                CheckoutTargetKind::Tag => {
                    let obj = repo
                        .revparse_single(&format!("refs/tags/{source}"))
                        .or_else(|_| repo.revparse_single(source))
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    (
                        obj.peel_to_commit()
                            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?,
                        format!("tag '{source}'"),
                    )
                }
                CheckoutTargetKind::Commit => {
                    let c = resolve_commit(repo, source)?;
                    let desc = format!("commit '{}'", short_id(c.id()));
                    (c, desc)
                }
            };
            let their_oid = their_commit.id();
            let log_label = match opts.kind {
                CheckoutTargetKind::Branch => {
                    local_branch_name(repo, source).unwrap_or_else(|_| source.to_string())
                }
                CheckoutTargetKind::Tag => source.to_string(),
                CheckoutTargetKind::Commit => short_id(their_oid),
            };

            let head_commit = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let head_oid = head_commit.id();

            let annotated = repo
                .find_annotated_commit(their_oid)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let (analysis, _) = repo
                .merge_analysis(&[&annotated])
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            if analysis.is_up_to_date() {
                return Ok(out);
            }

            // 提交消息：自定义优先；否则 git 惯例格式 + 可选被合并侧日志（merge.log）
            let auto_msg = |repo: &Repository| -> Result<String> {
                let base = format!("Merge {source_desc}");
                if opts.log_messages == 0 {
                    return Ok(base);
                }
                let log = merge_log(repo, head_oid, their_oid, &log_label, opts.log_messages)?;
                Ok(format!("{base}\n\n{log}"))
            };
            let msg = match opts.message.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(m) => m.to_string(),
                None => auto_msg(repo)?,
            };

            // -s ours：结果树取当前分支（无内容变化、无冲突），仅记录合并关系；
            // 永不快进（与 git 一致），ff-only 下报错
            if opts.strategy == "ours" {
                if opts.ff_only {
                    return Err(HigitError::new(
                        ErrorCode::Diverged,
                        "Not possible to fast-forward, aborting",
                    ));
                }
                repo.merge_base(head_oid, their_oid)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                if opts.squash {
                    // 无内容变化可暂存：等价 no-commit（不设 MERGE_HEAD）
                    repo.cleanup_state()
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    out.pending_commit = true;
                    return Ok(out);
                }
                if opts.no_commit {
                    repo.reference("MERGE_HEAD", their_oid, true, "merge -s ours")
                        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                    write_merge_msg(repo, &msg)?;
                    out.pending_commit = true;
                    return Ok(out);
                }
                let sig = merge_signature(repo)?;
                let oid = repo
                    .commit(
                        Some("HEAD"),
                        &sig,
                        &sig,
                        &msg,
                        &head_commit.tree().map_err(|e| {
                            HigitError::new(ErrorCode::Unknown, e.message())
                        })?,
                        &[&head_commit, &their_commit],
                    )
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                let _ = repo.cleanup_state();
                out.updated = true;
                out.created_commit = Some(oid.to_string());
                return Ok(out);
            }

            // 快进：未禁用且可行 → 快进完成（squash 不移动分支，只把变更写入暂存区）
            let can_ff = analysis.is_fast_forward();
            if can_ff && !opts.no_ff && !opts.squash {
                let mut co = git2::build::CheckoutBuilder::new();
                repo.checkout_tree(their_commit.as_object(), Some(&mut co))
                    .map_err(map_checkout_err)?;
                let branch = current_branch(repo)
                    .filter(|b| !b.is_empty())
                    .ok_or_else(|| {
                        HigitError::new(ErrorCode::Unknown, "HEAD is detached or unborn")
                    })?;
                let mut local_ref = repo
                    .find_reference(&format!("refs/heads/{branch}"))
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                local_ref
                    .set_target(their_oid, &format!("merge: fast-forward to {source}"))
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                out.fast_forward = true;
                out.updated = true;
                return Ok(out);
            }
            if !can_ff && opts.ff_only {
                return Err(HigitError::new(
                    ErrorCode::Diverged,
                    "Not possible to fast-forward, aborting",
                ));
            }

            // 三方合并：干净结束 → 视模式提交 / 挂起；冲突 → 保留冲突状态并列出文件（FR-5.8 P0）
            let mut merge_opts = git2::MergeOptions::new();
            let mut checkout = git2::build::CheckoutBuilder::new();
            repo.merge(&[&annotated], Some(&mut merge_opts), Some(&mut checkout))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            let mut index = repo
                .index()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            if index.has_conflicts() {
                // squash 冲突：git 不保留 MERGE_HEAD，清理合并状态
                if opts.squash {
                    let _ = repo.cleanup_state();
                }
                out.conflicts = list_conflicts(repo)?;
                return Ok(out);
            }

            if opts.squash {
                // squash：变更已在暂存区；不建合并提交、不设 MERGE_HEAD
                repo.cleanup_state()
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                out.pending_commit = true;
                return Ok(out);
            }
            if opts.no_commit {
                // 保留 repo.merge 写入的 MERGE_HEAD，写入 MERGE_MSG 供提交时预填
                write_merge_msg(repo, &msg)?;
                out.pending_commit = true;
                return Ok(out);
            }

            index
                .write()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let tree_oid = index
                .write_tree()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let tree = repo
                .find_tree(tree_oid)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let sig = merge_signature(repo)?;
            let oid = repo
                .commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &msg,
                    &tree,
                    &[&head_commit, &their_commit],
                )
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            // 清理合并状态（MERGE_HEAD / MERGE_MSG），避免污染下一次提交
            let _ = repo.cleanup_state();
            out.updated = true;
            out.created_commit = Some(oid.to_string());
            Ok(out)
        })
    }

    /// 写入 git 身份（FR-4.2）。
    fn set_identity(
        &self,
        session: &RepoSession,
        scope: IdentityScope,
        identity: &Identity,
    ) -> Result<()> {
        self.with_session(session, |_s, repo| {
            let mut config = match scope {
                IdentityScope::Repo => repo
                    .config()
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?,
                IdentityScope::Global => git2::Config::open_default()
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?,
            };
            config
                .set_str("user.name", &identity.name)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            config
                .set_str("user.email", &identity.email)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            Ok(())
        })
    }

    /// 读取全局身份（FR-10.3）：仅全局配置，不含仓库级。
    fn global_identity(&self) -> Result<Option<Identity>> {
        let config = git2::Config::open_default()
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        let name = config
            .get_string("user.name")
            .ok()
            .filter(|s| !s.is_empty());
        let email = config
            .get_string("user.email")
            .ok()
            .filter(|s| !s.is_empty());
        match (name, email) {
            (Some(name), Some(email)) => Ok(Some(Identity { name, email })),
            _ => Ok(None),
        }
    }

    /// 写入全局身份（FR-10.3）。
    fn set_global_identity(&self, identity: &Identity) -> Result<()> {
        let mut config = git2::Config::open_default()
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        config
            .set_str("user.name", &identity.name)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        config
            .set_str("user.email", &identity.email)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        Ok(())
    }

    /// 清除仓库级身份覆盖：键不存在时静默忽略，效果为回退到全局身份。
    fn clear_repo_identity(&self, session: &RepoSession) -> Result<()> {
        self.with_session(session, |_s, repo| {
            let mut config = repo
                .config()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            for key in ["user.name", "user.email"] {
                let _ = config.remove(key);
            }
            Ok(())
        })
    }

    /// 只读仓库级（.git/config）身份：仅取 Local 层的 user.name / user.email，
    /// 不回落全局——用于添加仓库时判断是否单独配置过。
    fn local_identity(&self, session: &RepoSession) -> Result<Option<Identity>> {
        self.with_session(session, |_s, repo| {
            let Ok(config) = repo.config() else {
                return Ok(None);
            };
            let (mut name, mut email) = (None, None);
            if let Ok(mut entries) = config.entries(Some(r"user\.(name|email)")) {
                while let Some(Ok(ce)) = entries.next() {
                    if ce.level() != git2::ConfigLevel::Local {
                        continue;
                    }
                    match ce.name() {
                        Ok("user.name") => name = ce.value().ok().map(|s| s.to_string()),
                        Ok("user.email") => email = ce.value().ok().map(|s| s.to_string()),
                        _ => {}
                    }
                }
            }
            match (name, email) {
                (Some(n), Some(e)) if !n.is_empty() && !e.is_empty() => {
                    Ok(Some(Identity { name: n, email: e }))
                }
                _ => Ok(None),
            }
        })
    }

    /// 读取远端 URL（仓库设置对话框）；远端不存在返回 None。
    fn remote_url(&self, session: &RepoSession, name: &str) -> Result<Option<String>> {
        self.with_session(session, |_s, repo| match repo.find_remote(name) {
            Ok(r) => {
                let url = r
                    .url()
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                Ok(Some(url.to_string()))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(HigitError::new(ErrorCode::Unknown, e.message())),
        })
    }

    /// 写入远端 URL；远端不存在时创建（仓库设置对话框）。
    fn set_remote_url(&self, session: &RepoSession, name: &str, url: &str) -> Result<()> {
        self.with_session(session, |_s, repo| {
            if repo.find_remote(name).is_ok() {
                repo.remote_set_url(name, url)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            } else {
                repo.remote(name, url)
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            }
            Ok(())
        })
    }
}

/// 归一化相对路径：统一 “/” 分隔（libgit2 要求）
fn normalize_rel(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

/// 会话相对路径：剥离 workdir 前缀并归一化
fn rel_of(s: &Git2Session, path: &Path) -> PathBuf {
    normalize_rel(path.strip_prefix(&s.workdir).unwrap_or(path))
}

/// 提交身份：显式给定优先，否则读仓库配置（含全局回退）；缺失报 `IdentityMissing`（FR-4.2）。
fn resolve_identity(repo: &Repository, author: Option<&Identity>) -> Result<Identity> {
    if let Some(id) = author {
        return Ok(id.clone());
    }
    let config = repo
        .config()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    let name = config
        .get_string("user.name")
        .ok()
        .filter(|s| !s.is_empty());
    let email = config
        .get_string("user.email")
        .ok()
        .filter(|s| !s.is_empty());
    match (name, email) {
        (Some(name), Some(email)) => Ok(Identity { name, email }),
        _ => Err(HigitError::identity_missing()),
    }
}

/// 写索引并把当前暂存区内容固化为 tree 对象（commit / commit_amend 共用）。
fn index_tree(repo: &Repository) -> Result<git2::Tree<'_>> {
    let mut index = repo
        .index()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    index
        .write()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    let tree_oid = index
        .write_tree()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    repo.find_tree(tree_oid)
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))
}

// ---------- M4：历史 / 分支辅助 ----------

/// 安全检出被拒（工作区改动 / 未跟踪文件会被覆盖）→ `DirtyWorktree`。
fn map_checkout_err(e: git2::Error) -> HigitError {
    if e.code() == git2::ErrorCode::Conflict {
        HigitError::new(ErrorCode::DirtyWorktree, e.message())
    } else {
        HigitError::new(ErrorCode::Unknown, e.message())
    }
}

/// 把工作区（含暂存区）的已跟踪修改 stash 起来，供 merge 检出携带。
/// 无改动 / 冲突状态（无法 stash）时返回 false。未跟踪文件不参与（与 git checkout -m 一致）。
fn stash_workdir(repo: &mut Repository) -> Result<bool> {
    // statuses 借用 &repo，须在 stash_save2（&mut）前结束，故先在块内算出 dirty
    let dirty = {
        let statuses = repo
            .statuses(None)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        let mut dirty = false;
        for s in statuses.iter() {
            let st = s.status();
            if st.intersects(git2::Status::CONFLICTED) {
                // 仓库处于冲突状态：无法 stash，交给后续安全检出自行报错
                return Ok(false);
            }
            if st.is_empty() || st == Status::WT_NEW || st == Status::IGNORED {
                continue;
            }
            dirty = true;
        }
        dirty
    };
    if !dirty {
        return Ok(false);
    }
    let sig = repo.signature().map_err(|_| HigitError::identity_missing())?;
    repo.stash_save2(&sig, None, None)
        .map(|_| true)
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))
}

/// 远端分支引用名（如 "origin/feature/x"）对应的本地分支名：
/// 去掉已配置远端的前缀（git checkout --track 语义）；未匹配时取最后一段兜底。
fn local_branch_name(repo: &Repository, remote_ref: &str) -> Result<String> {
    let short = remote_ref.trim_start_matches("remotes/");
    let remotes = repo
        .remotes()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    for i in 0..remotes.len() {
        let Ok(Some(r)) = remotes.get(i) else { continue };
        if let Some(stripped) = short.strip_prefix(&format!("{r}/")) {
            return Ok(stripped.to_string());
        }
    }
    Ok(short.rsplit('/').next().unwrap_or(short).to_string())
}

/// 合并提交签名：取仓库配置的 user.name / user.email
fn merge_signature(repo: &Repository) -> Result<Signature<'_>> {
    let config = repo
        .config()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    let name = config
        .get_string("user.name")
        .ok()
        .filter(|s| !s.is_empty());
    let email = config
        .get_string("user.email")
        .ok()
        .filter(|s| !s.is_empty());
    match (name, email) {
        (Some(n), Some(e)) => Signature::now(&n, &e)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message())),
        _ => Err(HigitError::identity_missing()),
    }
}

/// merge.log：被合并侧（base..theirs）最近 limit 条提交摘要，git 惯例的短日志段落格式
fn merge_log(
    repo: &Repository,
    base: Oid,
    theirs: Oid,
    label: &str,
    limit: u32,
) -> Result<String> {
    let mut revwalk = repo
        .revwalk()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    revwalk.push(theirs).map_err(|e| {
        HigitError::new(ErrorCode::Unknown, e.message())
    })?;
    revwalk.hide(base).map_err(|e| {
        HigitError::new(ErrorCode::Unknown, e.message())
    })?;
    revwalk
        .set_sorting(Sort::TIME | Sort::TOPOLOGICAL)
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    let mut lines = vec![format!("* {label}:")];
    for oid in revwalk.take(limit as usize) {
        let Ok(oid) = oid else { continue };
        let Ok(c) = repo.find_commit(oid) else { continue };
        let summary = c.summary().ok().flatten().unwrap_or_default();
        lines.push(format!("  {summary}"));
    }
    Ok(lines.join("\n"))
}

/// 写入 MERGE_MSG（--no-commit 挂起时的预填消息）；git2 未绑定 message_set，直接写文件
fn write_merge_msg(repo: &Repository, msg: &str) -> Result<()> {
    let path = repo.path().join("MERGE_MSG");
    std::fs::write(&path, format!("{msg}\n"))
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.to_string()))
}

/// commit_partial / commit_amend_partial 共用：按路径拼树后落提交。
/// amend=false 在 HEAD 之上新提交（空提交拒绝、MERGE_HEAD 双亲语义）；
/// amend=true 替换 HEAD（父提交沿用原提交，仅改消息的空树修补合法）。
fn partial_commit_impl(
    repo: &Repository,
    workdir: &Path,
    message: &str,
    sig: &Signature<'_>,
    paths: &[String],
    amend: bool,
) -> Result<String> {
    let err = |e: git2::Error| HigitError::new(ErrorCode::Unknown, e.message());
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    if amend && head.is_none() {
        return Err(HigitError::new(ErrorCode::Unknown, "no commit to amend"));
    }
    // 勾选路径先按工作区内容暂存：只复制仓库索引条目的话，未暂存文件取到的是
    // HEAD 旧内容，拼出的树与 HEAD 相同，会产生空提交
    let rels: Vec<PathBuf> = paths
        .iter()
        .map(|p| PathBuf::from(p.replace('\\', "/")))
        .collect();
    let mut repo_index = repo.index().map_err(err)?;
    for rel in &rels {
        if workdir.join(rel).is_file() {
            repo_index.add_path(rel).map_err(err)?;
        } else {
            // 工作区已无此文件：暂存删除；索引里也没有则忽略
            let _ = repo_index.remove_path(rel);
        }
    }
    repo_index.write().map_err(err)?;

    let mut tmp = git2::Index::new().map_err(err)?;
    if let Some(c) = &head {
        tmp.read_tree(&c.tree().map_err(err)?).map_err(err)?;
    }
    for rel in &rels {
        match repo_index.get_path(rel, 0) {
            Some(entry) => tmp.add(&entry).map_err(err)?,
            // 暂存删除：索引已无此条目，从基树中移除
            None => tmp.remove_path(rel).map_err(err)?,
        }
    }
    let tree_oid = tmp.write_tree_to(repo).map_err(err)?;
    // 拼出的树与 HEAD 相同说明无变更可提交，拒绝（git 默认不产生空提交）；
    // amend 仅改消息是合法操作，不拒绝
    if let Some(c) = &head {
        if tree_oid == c.tree_id() && !amend && repo.find_reference("MERGE_HEAD").is_err() {
            return Err(HigitError::new(ErrorCode::Unknown, "nothing to commit"));
        }
    }
    let tree = repo.find_tree(tree_oid).map_err(err)?;

    if amend {
        let head = head.as_ref().unwrap();
        let parents: Vec<git2::Commit> = head.parents().collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        // amend 的第一亲代是 HEAD~1，git_commit_create(update_ref=HEAD) 会因
        // "first parent 不是当前 tip" 拒绝：先只建提交对象，再手动前移分支引用
        let oid = repo
            .commit(None, sig, sig, message, &tree, &parent_refs)
            .map_err(err)?;
        let mut head_ref = repo
            .head()
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        head_ref.set_target(oid, "commit: amend").map_err(err)?;
        return Ok(oid.to_string());
    }

    let mut parents: Vec<git2::Commit> = head.into_iter().collect();
    // 合并收尾时按路径提交：保留 MERGE_HEAD 双亲语义并清理合并状态
    let merge_head = repo.find_reference("MERGE_HEAD").ok();
    if let Some(mh) = &merge_head {
        if let Ok(p) = mh.peel_to_commit() {
            parents.push(p);
        }
    }
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), sig, sig, message, &tree, &parent_refs)
        .map_err(err)?;
    if merge_head.is_some() {
        let _ = repo.cleanup_state();
    }
    Ok(oid.to_string())
}

/// 短 hash（7 位，与 git 默认一致）
fn short_id(oid: Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

/// 解析提交标识：完整/短 hash、分支名、HEAD 表达式均可（revparse_single）。
fn resolve_commit<'a>(repo: &'a Repository, spec: &str) -> Result<git2::Commit<'a>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    obj.peel_to_commit()
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))
}

/// 从 diff 提取行序列（统一视图）：供 diff_text 与 commit_file_diff 共用。
/// 命中二进制时 libgit2 调用 binary_cb 而非行回调（FR-8.4）。
fn extract_diff_lines(diff: &git2::Diff) -> Result<TextDiff> {
    let stats = diff
        .stats()
        .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;

    let lines = Arc::new(Mutex::new(Vec::<DiffLine>::new()));
    let is_binary = Arc::new(AtomicBool::new(false));
    let truncated = Arc::new(AtomicBool::new(false));

    let lines_file = lines.clone();
    let truncated_flag = truncated.clone();
    let mut file_cb = |_delta: git2::DiffDelta, _progress: f32| -> bool { true };
    let is_binary_flag = is_binary.clone();
    let mut binary_cb = |_delta: git2::DiffDelta, _binary: git2::DiffBinary| -> bool {
        is_binary_flag.store(true, Ordering::SeqCst);
        false
    };
    let mut hunk_cb = |_delta: git2::DiffDelta, hunk: git2::DiffHunk| -> bool {
        if let Ok(mut buf) = lines_file.lock() {
            buf.push(DiffLine {
                origin: DiffLineOrigin::HunkHeader,
                text: String::from_utf8_lossy(hunk.header())
                    .trim_end()
                    .to_string(),
            });
        }
        true
    };
    let mut line_cb =
        |_delta: git2::DiffDelta, _hunk: Option<git2::DiffHunk>, line: git2::DiffLine| -> bool {
            let Ok(mut buf) = lines_file.lock() else {
                return false;
            };
            if buf.len() >= DIFF_LINE_CAP {
                truncated_flag.store(true, Ordering::SeqCst);
                return false;
            }
            let origin = match line.origin() {
                '+' => DiffLineOrigin::Addition,
                '-' => DiffLineOrigin::Deletion,
                'H' | 'F' => return true, // 文件头，不展示
                _ => DiffLineOrigin::Context,
            };
            let text = String::from_utf8_lossy(line.content());
            buf.push(DiffLine {
                origin,
                text: text.trim_end_matches('\n').to_string(),
            });
            true
        };

    // 达到行数上限时回调返回 false 中止迭代，libgit2 以 EUSER 报错——属预期
    let _ = diff.foreach(
        &mut file_cb,
        Some(&mut binary_cb),
        Some(&mut hunk_cb),
        Some(&mut line_cb),
    );
    let lines_out = lines.lock().ok().map(|b| b.clone()).unwrap_or_default();

    Ok(TextDiff {
        lines: lines_out,
        additions: stats.insertions(),
        deletions: stats.deletions(),
        is_binary: is_binary.load(Ordering::SeqCst),
        truncated: truncated.load(Ordering::SeqCst),
        too_large: false,
    })
}

/// 提交 diff → 变更文件列表：file_cb 记录当前 delta 序号，line_cb 按序号累计行数。
fn collect_commit_files(diff: &mut git2::Diff, out: &mut Vec<CommitFile>) -> Result<()> {
    use std::cell::Cell;
    let count = diff.deltas().len();
    let mut adds = vec![0usize; count];
    let mut dels = vec![0usize; count];
    let cur = Cell::new(0usize);

    let mut file_cb = |_delta: git2::DiffDelta, _progress: f32| -> bool {
        cur.set(cur.get() + 1);
        true
    };
    let mut line_cb =
        |_delta: git2::DiffDelta, _hunk: Option<git2::DiffHunk>, line: git2::DiffLine| -> bool {
            let idx = cur.get().saturating_sub(1);
            match line.origin() {
                '+' => adds[idx] += 1,
                '-' => dels[idx] += 1,
                _ => {}
            }
            true
        };
    let _ = diff.foreach(&mut file_cb, None, None, Some(&mut line_cb));

    for (i, delta) in diff.deltas().enumerate() {
        let old = delta.old_file().path().map(PathBuf::from);
        let new = delta.new_file().path().map(PathBuf::from);
        let kind = match delta.status() {
            Delta::Added => CommitFileKind::Added,
            Delta::Deleted => CommitFileKind::Deleted,
            Delta::Renamed => CommitFileKind::Renamed,
            _ => CommitFileKind::Modified,
        };
        let Some(path) = new.or(old.clone()) else {
            continue;
        };
        out.push(CommitFile {
            path,
            old_path: if kind == CommitFileKind::Renamed {
                old
            } else {
                None
            },
            kind,
            additions: adds[i],
            deletions: dels[i],
        });
    }
    Ok(())
}

/// 冲突文件列表（FR-5.8 P0）：来自状态扫描的 CONFLICTED 条目。
fn list_conflicts(repo: &Repository) -> Result<Vec<PathBuf>> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false);
    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| HigitError::new(ErrorCode::StatusFailed, e.message()))?;
    let mut out = Vec::new();
    for item in statuses.iter() {
        if item.status().intersects(Status::CONFLICTED) {
            if let Ok(p) = item.path() {
                out.push(PathBuf::from(p));
            }
        }
    }
    Ok(out)
}

// ---------- M3：网络同步辅助 ----------

/// 组装远端回调：传输进度 + 取消 + 凭证 + SSH 主机密钥（FR-5.5/5.6/5.10）。
fn build_callbacks<'a>(
    creds: Option<&Credentials>,
    url: &str,
    progress: &'a dyn Fn(Progress),
    cancel: &CancelToken,
) -> git2::RemoteCallbacks<'a> {
    let mut cbs = git2::RemoteCallbacks::new();
    {
        let cancel = cancel.clone();
        cbs.transfer_progress(move |stats| {
            // 接收阶段按对象计数；解析阶段按 delta 计数（对象计数此时恒等，百分比无变化）
            let (stage, done, total) = if stats.received_objects() < stats.total_objects() {
                (
                    "Receiving objects",
                    stats.received_objects() as u64,
                    stats.total_objects() as u64,
                )
            } else {
                (
                    "Resolving deltas",
                    stats.indexed_deltas() as u64,
                    stats.total_deltas() as u64,
                )
            };
            progress(Progress { stage, done, total });
            !cancel.is_cancelled()
        });
    }
    // 未配置凭证时也装上 SSH 默认链路（ssh-agent → 默认密钥），
    // 否则 SSH 远端必然 "authentication required but no callback set"
    apply_credentials(
        &mut cbs,
        creds.cloned().unwrap_or(Credentials::SshKey {
            username: String::new(),
            key_path: String::new(),
            passphrase: String::new(),
            from_store: false,
        }),
        None,
    );
    set_hostkey_callback(&mut cbs, url);
    cbs
}

/// SSH 主机密钥校验（TOFU）：未知主机记录进 known_hosts 后放行，已记录密钥变更硬拒绝；
/// HTTPS 走 `CertificatePassthrough` 交还 libgit2 内建证书校验，行为不变。
fn set_hostkey_callback(cbs: &mut git2::RemoteCallbacks<'_>, url: &str) {
    let host_port = ssh::host_and_port(url);
    cbs.certificate_check(move |cert, _host| {
        let Some(hk) = cert.as_hostkey() else {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        };
        let Some((host, port)) = &host_port else {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        };
        let Some(blob) = hk.hostkey() else {
            return Ok(git2::CertificateCheckStatus::CertificateOk);
        };
        let key_type = hk
            .hostkey_type()
            .map(|t| t.name().to_string())
            .unwrap_or_default();
        let marker = ssh::host_marker(host, *port);
        let Some(home) = home_dir() else {
            // 无法定位 known_hosts：不持久化，仅本次放行
            return Ok(git2::CertificateCheckStatus::CertificateOk);
        };
        let known = home.join(".ssh").join("known_hosts");
        match ssh::check(&known, &marker, &key_type, blob) {
            ssh::HostKeyVerdict::Match => Ok(git2::CertificateCheckStatus::CertificateOk),
            ssh::HostKeyVerdict::Unknown => {
                // 未知算法不落盘（无法生成合法 known_hosts 条目），仅本次放行
                if !key_type.is_empty() && key_type != "unknown" {
                    let line = format!("{marker} {key_type} {}", ssh::b64_encode(blob));
                    let _ = ssh::append(&known, &line);
                }
                Ok(git2::CertificateCheckStatus::CertificateOk)
            }
            ssh::HostKeyVerdict::Mismatch => Err(git2::Error::new(
                git2::ErrorCode::GenericError,
                git2::ErrorClass::Ssh,
                format!(
                    "SSH host key mismatch for {host}: the server key changed or the connection is being intercepted"
                ),
            )),
        }
    });
}

/// 是否存在可用的 SSH agent（Windows：OpenSSH agent 命名管道或 Pageant；Unix：SSH_AUTH_SOCK）。
/// 探测结果只用于决定链路中是否包含 Agent 步骤。
#[cfg(windows)]
fn ssh_agent_available() -> bool {
    // Windows OpenSSH agent 命名管道
    if std::fs::OpenOptions::new()
        .read(true)
        .open(r"\\.\pipe\openssh-ssh-agent")
        .is_ok()
    {
        return true;
    }
    // Pageant（PuTTY 认证代理）主窗口
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowA;
    unsafe { !FindWindowA(c"Pageant".as_ptr().cast(), std::ptr::null()).is_null() }
}

#[cfg(not(windows))]
fn ssh_agent_available() -> bool {
    std::env::var_os("SSH_AUTH_SOCK").is_some_and(|s| !s.is_empty())
}

/// SSH 认证链步骤。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SshStep {
    /// 仓库设置 / 凭证对话框显式配置的密钥
    Configured,
    /// ssh-agent（Windows：OpenSSH agent / Pageant；Unix：SSH_AUTH_SOCK）
    Agent,
    /// ~/.ssh 默认密钥（id_ed25519 / id_ecdsa / id_rsa）
    Default,
}

/// SSH 认证链路：显式配置的密钥优先于 ssh-agent，最后回退 ~/.ssh 默认密钥。
/// 配置的密钥若是当前加密后端无法解析的格式（见 [`ssh_key_backend_supported`]），不进入
/// 链路——libgit2 对解析失败会直接终止会话且不再回调凭证链，链路将无法回退 agent。
fn ssh_steps(configured: bool, agent_available: bool) -> &'static [SshStep] {
    match (configured, agent_available) {
        (true, true) => &[SshStep::Configured, SshStep::Agent, SshStep::Default],
        (true, false) => &[SshStep::Configured, SshStep::Default],
        (false, true) => &[SshStep::Agent, SshStep::Default],
        (false, false) => &[SshStep::Default],
    }
}

/// 显式配置的 SSH 密钥能否被当前 libgit2 加密后端解析。
///
/// libgit2 的 libssh2 私钥解析取决于编译时的加密后端，三平台已统一为 OpenSSL：
/// Windows 经 openssl-on-win32 开启（见 higit-git Cargo.toml，默认的 WinCNG 后端
/// 只认 PKCS#1 的 RSA / DSA），Unix 由 vendored-openssl 提供。OpenSSL 后端可解析
/// OpenSSH 容器（ed25519 等新格式）、PKCS#8、EC 与 PKCS#1 PEM，唯一没有解析器的
/// 是 PuTTY .ppk 容器。.ppk 交给 libgit2 后，解析失败会直接终止会话且不再回调凭证链
/// （见 [`apply_credentials`] 注释），链路无法回退 agent——因此按文件头嗅探格式，
/// 不支持的直接跳过 Configured 步（同步对话框会提示降级到 ssh-agent）。
///
/// 文件不存在 / 不可读时返回 true：凭证构造会失败并正常进入链路下一步。
pub fn ssh_key_backend_supported(key_path: &str) -> bool {
    ssh_key_backend_issue(key_path).is_none()
}

/// 密钥格式不受当前引擎支持的具体原因（None = 可用或无法判断）。
/// 原因文案直接进同步对话框日志，指引用户转换成受支持的格式。
fn ssh_key_backend_issue(key_path: &str) -> Option<&'static str> {
    use std::io::Read;
    let mut buf = [0u8; 64];
    let n = std::fs::File::open(key_path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    if n == 0 {
        return None;
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    if head.contains("PuTTY-User-Key-File") {
        // PuTTY 私钥容器（.ppk），libssh2 的 OpenSSL / WinCNG 后端均无解析器
        Some("PuTTY .ppk 格式不受当前引擎支持，请用 PuTTYgen 导出为 OpenSSH / PEM 格式")
    } else {
        None
    }
}

/// 凭证链步骤日志（同步对话框详细日志）：每一步验证的尝试 / 结果实时上报到 `on_event`；
/// `last` 记录最近一次交给远端的凭证描述，供"被远端拒绝"与"认证通过"事件使用。
struct AuthLog<'a> {
    emit: &'a (dyn Fn(SyncEvent) + Send + Sync),
    last: std::sync::Mutex<Option<String>>,
}

impl<'a> AuthLog<'a> {
    fn new(emit: &'a (dyn Fn(SyncEvent) + Send + Sync)) -> Self {
        Self {
            emit,
            last: std::sync::Mutex::new(None),
        }
    }

    fn log(&self, level: &str, text: String) {
        (self.emit)(SyncEvent::Log {
            level: level.to_string(),
            text,
        });
    }

    /// 报告一次凭证尝试并记录其描述（用于后续"被远端拒绝" / "认证通过"）。
    fn attempt(&self, desc: String) {
        self.log("dim", format!("认证: 尝试 {desc}"));
        *self.last.lock().expect("auth log mutex poisoned") = Some(desc);
    }

    /// 上一次交给远端的凭证被远端拒绝（凭证回调被再次调用）。
    fn reject_last(&self) {
        if let Some(d) = self.last.lock().expect("auth log mutex poisoned").take() {
            self.log("warn", format!("认证: {d} 被远端拒绝"));
        }
    }

    /// 网络操作成功：最后一次尝试的方式即通过方式。
    fn succeed(&self) {
        if let Some(d) = self.last.lock().expect("auth log mutex poisoned").take() {
            self.log("ok", format!("认证通过: {d}"));
        }
    }
}

/// 拉取整合方案（pull_fetch_plan 依据本地/远端关系决定）
enum PullPlan {
    UpToDate,
    FastForward { up_oid: Oid, up_name: String },
    Merge { up_name: String },
    Rebase { up_name: String, up_oid: Oid },
}

/// 同步过程日志：on_event 缺省时静默
fn sync_logger<'a>(
    on_event: Option<&'a (dyn Fn(SyncEvent) + Send + Sync)>,
) -> impl Fn(&str, String) + 'a {
    move |level: &str, text: String| {
        if let Some(ev) = on_event {
            ev(SyncEvent::Log {
                level: level.to_string(),
                text,
            });
        }
    }
}

/// 当前 HEAD 的摘要信息（快进 / 合并 / 变基完成后回填 PullOutcome）
fn head_info_of(repo: &Repository) -> Result<HeadInfo> {
    let c = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
    Ok(HeadInfo {
        short_id: short_id(c.id()),
        summary: c.summary().ok().flatten().unwrap_or_default().to_string(),
        time_unix: c.time().seconds(),
        committer_email: c.committer().email().unwrap_or("").to_string(),
    })
}

/// 凭证回调：HTTPS 走用户名/密码；SSH 按链路逐次尝试 指定密钥（格式受支持时优先）→
/// ssh-agent → ~/.ssh 默认密钥。每一步的尝试 / 拒绝 / 跳过经 `auth` 实时上报（详细日志模式）。
/// libgit2 每次认证失败都会重新调用回调，用计数器步进链路；链路耗尽立即返回 Auth 错误，
/// 避免对同一无效凭证反复重试（实测可空转 200+ 次），同时保持 AuthFailed 分类驱动凭证对话框。
fn apply_credentials<'a>(
    cbs: &mut git2::RemoteCallbacks<'a>,
    creds: Credentials,
    auth: Option<&'a AuthLog<'a>>,
) {
    let (userpass, ssh) = match &creds {
        Credentials::UserPass { username, password } => {
            (Some((username.clone(), password.clone())), None)
        }
        Credentials::SshKey {
            username,
            key_path,
            passphrase,
            from_store,
        } => {
            let agent_available = ssh_agent_available();
            // 日志里的凭证来源：仓库设置 / 对话框显式配置 vs 系统凭证管理器"记住"的凭证
            let origin = if *from_store { "记住的密钥" } else { "配置的密钥" };
            let issue = (!key_path.is_empty())
                .then(|| ssh_key_backend_issue(key_path))
                .unwrap_or(None);
            let configured = issue.is_none();
            // 配置了密钥但引擎后端无法解析：显式提示降级（同步对话框日志）
            if let (Some(a), Some(reason)) = (auth.as_ref(), issue) {
                a.log("warn", format!("认证: 跳过 {origin} {key_path}（{reason}）"));
            }
            (
                None,
                Some((
                    username.clone(),
                    key_path.clone(),
                    passphrase.clone(),
                    origin,
                    ssh_steps(configured, agent_available),
                )),
            )
        }
    };
    let next = std::cell::Cell::new(0usize);
    cbs.credentials(move |_url, username_from_url, allowed| {
        let cfg_user = ssh
            .as_ref()
            .map(|(u, ..)| u.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                userpass
                    .as_ref()
                    .map(|(u, _)| u.as_str())
                    .filter(|s| !s.is_empty())
            });
        // URL 未带用户名时的用户名询问：不属于认证失败，不消耗链路步数
        if !allowed.intersects(CredentialType::SSH_KEY | CredentialType::USER_PASS_PLAINTEXT) {
            let name = cfg_user
                .or(username_from_url)
                .filter(|s| !s.is_empty())
                .unwrap_or("git");
            return Cred::username(name);
        }
        // 走到这里是一次真实凭证请求：若此前已交出过凭证，说明它被远端拒绝
        if let Some(a) = auth {
            a.reject_last();
        }
        if let Some((u, p)) = &userpass {
            if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
                if let Some(a) = auth {
                    a.attempt(format!("HTTPS 用户名 / 密码（用户 {u}）"));
                }
                return Cred::userpass_plaintext(u, p);
            }
        }
        if !allowed.contains(CredentialType::SSH_KEY) {
            // 无可用凭证类型：立即按认证失败结束（驱动 keyring 重试 / 凭证对话框）
            return Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Ssh,
                "authentication required but no usable credential",
            ));
        }
        let Some((_, key_path, passphrase, origin, steps)) = &ssh else {
            return Cred::default();
        };
        let name = cfg_user
            .or(username_from_url)
            .filter(|s| !s.is_empty())
            .unwrap_or("git")
            .to_string();
        let pass = (!passphrase.is_empty()).then_some(passphrase.as_str());
        for (idx, step) in steps.iter().enumerate().skip(next.get()) {
            let cred: Option<(Cred, String)> = match step {
                SshStep::Configured => match Cred::ssh_key(&name, None, Path::new(key_path), pass)
                {
                    Ok(c) => Some((c, format!("{origin} {key_path}"))),
                    Err(_) => {
                        if let Some(a) = auth {
                            a.log("dim", format!("认证: 跳过 {origin} {key_path}（不可用）"));
                        }
                        None
                    }
                },
                SshStep::Agent => match Cred::ssh_key_from_agent(&name) {
                    Ok(c) => Some((c, "ssh-agent".to_string())),
                    Err(_) => {
                        if let Some(a) = auth {
                            a.log("dim", "认证: 跳过 ssh-agent（无可用凭证）".into());
                        }
                        None
                    }
                },
                SshStep::Default => {
                    if let Some(home) = home_dir() {
                        let mut found: Option<(Cred, String)> = None;
                        for f in ["id_ed25519", "id_ecdsa", "id_rsa"] {
                            let p = home.join(".ssh").join(f);
                            if !p.exists() {
                                continue;
                            }
                            // 默认密钥同样可能是引擎解析不了的格式：解析失败会终止
                            // 会话且不再回调，必须先嗅探跳过才能落到后面的默认密钥
                            if let Some(reason) = ssh_key_backend_issue(&p.to_string_lossy()) {
                                if let Some(a) = auth {
                                    a.log(
                                        "dim",
                                        format!("认证: 跳过 默认密钥 ~/.ssh/{f}（{reason}）"),
                                    );
                                }
                                continue;
                            }
                            let desc = format!("默认密钥 ~/.ssh/{f}");
                            match Cred::ssh_key(&name, None, &p, None) {
                                Ok(c) => {
                                    found = Some((c, desc));
                                    break;
                                }
                                Err(_) => {
                                    if let Some(a) = auth {
                                        a.log("dim", format!("认证: 跳过 {desc}（不可用）"));
                                    }
                                }
                            }
                        }
                        if found.is_none() {
                            if let Some(a) = auth {
                                a.log("dim", "认证: ~/.ssh 无可用默认密钥".into());
                            }
                        }
                        found
                    } else {
                        if let Some(a) = auth {
                            a.log("dim", "认证: 跳过 默认密钥（无法定位用户主目录）".into());
                        }
                        None
                    }
                }
            };
            if let Some((c, desc)) = cred {
                if let Some(a) = auth {
                    a.attempt(desc);
                }
                next.set(idx + 1);
                return Ok(c);
            }
        }
        if let Some(a) = auth {
            a.log("error", "认证: 所有验证方式均被远端拒绝".into());
        }
        // 链路耗尽：立即按认证失败结束（驱动 keyring 重试 / 凭证对话框）
        Err(git2::Error::new(
            git2::ErrorCode::Auth,
            git2::ErrorClass::Ssh,
            "SSH authentication failed: configured key, ssh-agent and default keys were all rejected",
        ))
    });
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// 当前分支名：HEAD 符号目标去掉 refs/heads/ 前缀；未出生分支与普通分支都适用。
fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.find_reference("HEAD").ok()?;
    let target = head.symbolic_target().ok().flatten()?;
    let name = target.trim_start_matches("refs/heads/");
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// 解析远端名：显式名优先；否则 上游所属远端 → origin → 第一个远端（FR-5.2）。
fn resolve_remote_name(repo: &Repository, remote: &str, branch: Option<&str>) -> Result<String> {
    if !remote.is_empty() {
        return Ok(remote.to_string());
    }
    if let Some(b) = branch {
        if let Ok(local) = repo.find_branch(b, git2::BranchType::Local) {
            if let Ok(up) = local.upstream() {
                if let Ok(Some(up_name)) = up.name() {
                    if let Some((remote_name, _)) = up_name.split_once('/') {
                        if repo.find_remote(remote_name).is_ok() {
                            return Ok(remote_name.to_string());
                        }
                    }
                }
            }
        }
    }
    if repo.find_remote("origin").is_ok() {
        return Ok("origin".to_string());
    }
    if let Ok(list) = repo.remotes() {
        if let Some(first) = list.iter().flatten().flatten().next() {
            return Ok(first.to_string());
        }
    }
    Err(HigitError::new(
        ErrorCode::NetworkFailed,
        "no remotes configured",
    ))
}

/// URL → 认证上下文（凭证对话框按它填充字段）。
fn auth_detail_from_url(url: &str) -> Option<AuthDetail> {
    let (kind, rest) = if let Some(r) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        (AuthKind::Http, r)
    } else if let Some(r) = url.strip_prefix("ssh://") {
        (AuthKind::Ssh, r)
    } else {
        // git@host:path
        let rest = url.strip_prefix("git@")?;
        let host = rest.split([':', '/']).next().unwrap_or_default();
        return Some(AuthDetail {
            kind: AuthKind::Ssh,
            host: host.to_string(),
            username: "git".to_string(),
        });
    };

    // [user@]host[:port][/path]
    let (userinfo, host_port) = match rest.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    let host = host_port
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();
    Some(AuthDetail {
        kind,
        host,
        username: userinfo.unwrap_or_default().to_string(),
    })
}

/// 网络错误归类（FR-5.7 / FR-5.10）：取消 → 认证 → 推送被拒 → 网络失败。
/// `had_creds` = 本次尝试是否携带凭证：未携带时 HTTP 401/404 也归为认证失败——
/// GitHub 对私有仓库的无凭证请求返回 404，不归类则永远无法触发凭证流程。
fn map_net_err(e: git2::Error, url: &str, cancelled: bool, had_creds: bool) -> HigitError {
    if cancelled {
        return HigitError::cancelled();
    }
    let msg = e.message();
    let lower = msg.to_lowercase();
    let unauth_http_status = !had_creds
        && (url.starts_with("http://") || url.starts_with("https://"))
        && (lower.contains("status code 401") || lower.contains("status code 404"));
    if e.code() == git2::ErrorCode::Auth || unauth_http_status {
        let detail = auth_detail_from_url(url).unwrap_or(AuthDetail {
            kind: AuthKind::Http,
            host: String::new(),
            username: String::new(),
        });
        return HigitError::auth_failed(detail, msg);
    }
    if lower.contains("fetch first")
        || lower.contains("non-fast-forward")
        // libgit2 本地预检文案（远端引用非本地提交的后代）
        || lower.contains("non-fastforwardable")
        || lower.contains("rejected")
        || lower.contains("stale info")
    {
        return HigitError::new(ErrorCode::PushRejected, msg);
    }
    if e.code() == git2::ErrorCode::GenericError && lower.contains("authentic") {
        let detail = auth_detail_from_url(url).unwrap_or(AuthDetail {
            kind: AuthKind::Http,
            host: String::new(),
            username: String::new(),
        });
        return HigitError::auth_failed(detail, msg);
    }
    // 网络错误附加 libgit2 错误类别 / 错误码，便于区分 DNS / TLS / 代理 / 协议层问题
    HigitError::new(
        ErrorCode::NetworkFailed,
        format!("{msg} (libgit2: class={:?} code={:?})", e.class(), e.code()),
    )
}

/// 把 libgit2 的状态位折叠为单一 [`FileStatusKind`]。
/// 同一文件可能同时带 INDEX_* 与 WT_* 位（已暂存后又修改）：
/// M0 先归为 Staged，M2 引入“已暂存 + 工作区再修改”的细分展示。
fn map_status(status: Status, include_ignored: bool) -> Option<FileStatusKind> {
    if status.intersects(
        Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE,
    ) {
        return Some(FileStatusKind::Staged);
    }
    if status.intersects(Status::WT_MODIFIED | Status::WT_TYPECHANGE) {
        return Some(FileStatusKind::Modified);
    }
    if status.intersects(Status::WT_DELETED) {
        return Some(FileStatusKind::Deleted);
    }
    if status.intersects(Status::WT_RENAMED) {
        return Some(FileStatusKind::Renamed);
    }
    if status.intersects(Status::CONFLICTED) {
        return Some(FileStatusKind::Conflicted);
    }
    if status.intersects(Status::WT_NEW) {
        return Some(FileStatusKind::Untracked);
    }
    if include_ignored && status.intersects(Status::IGNORED) {
        return Some(FileStatusKind::Ignored);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_net_err_appends_libgit2_class_and_code() {
        let e = git2::Error::from_str("failed to connect to example.com");
        let err = map_net_err(e, "https://example.com/user/repo.git", false, true);
        assert_eq!(err.code, ErrorCode::NetworkFailed);
        let raw = err.raw.unwrap_or_default();
        assert!(raw.contains("failed to connect to example.com"));
        assert!(raw.contains("libgit2: class="));
        assert!(raw.contains("code="));
    }

    #[test]
    fn auth_detail_parses_common_urls() {
        let d = auth_detail_from_url("https://github.com/user/repo.git").unwrap();
        assert_eq!(d.kind, AuthKind::Http);
        assert_eq!(d.host, "github.com");
        assert_eq!(d.username, "");

        let d = auth_detail_from_url("https://alice@github.com/user/repo.git").unwrap();
        assert_eq!(d.kind, AuthKind::Http);
        assert_eq!(d.host, "github.com");
        assert_eq!(d.username, "alice");

        let d = auth_detail_from_url("git@github.com:hicode0101/HiGit.git").unwrap();
        assert_eq!(d.kind, AuthKind::Ssh);
        assert_eq!(d.host, "github.com");
        assert_eq!(d.username, "git");

        let d = auth_detail_from_url("ssh://bob@gitlab.com:2222/user/repo.git").unwrap();
        assert_eq!(d.kind, AuthKind::Ssh);
        assert_eq!(d.host, "gitlab.com");
        assert_eq!(d.username, "bob");

        assert!(auth_detail_from_url("D:\\Code\\repo").is_none());
        assert!(auth_detail_from_url("/srv/git/repo").is_none());
    }

    #[test]
    fn map_net_err_flags_unauthenticated_http_404_as_auth() {
        // GitHub 对私有仓库的无凭证请求返回 404：未携带凭证 → 认证失败（触发凭证流程）
        let e = git2::Error::from_str("request failed with status code 404");
        let err = map_net_err(e, "https://github.com/user/repo.git", false, false);
        assert_eq!(err.code, ErrorCode::AuthFailed);
        assert_eq!(err.auth.unwrap().host, "github.com");

        // 已携带凭证时的 404 = 仓库不存在 / 无权限：保持网络失败，避免凭证框死循环
        let e = git2::Error::from_str("request failed with status code 404");
        let err = map_net_err(e, "https://github.com/user/repo.git", false, true);
        assert_eq!(err.code, ErrorCode::NetworkFailed);

        // 401 同理
        let e = git2::Error::from_str("request failed with status code 401");
        let err = map_net_err(e, "https://github.com/user/repo.git", false, false);
        assert_eq!(err.code, ErrorCode::AuthFailed);
    }

    #[test]
    fn map_net_err_classifies_push_rejections() {
        // libgit2 本地预检 + 服务端拒绝两种文案都要归为推送被拒
        for msg in [
            "cannot push non-fastforwardable reference",
            "remote rejected: non-fast-forward",
        ] {
            let e = git2::Error::from_str(msg);
            let err = map_net_err(e, "git@github.com:user/repo.git", false, true);
            assert_eq!(err.code, ErrorCode::PushRejected, "{msg}");
        }
    }

    #[test]
    fn ssh_steps_configured_key_before_agent() {
        // 显式配置的密钥优先于 ssh-agent；agent 可用性只决定是否包含 Agent 步
        use SshStep::*;
        assert_eq!(ssh_steps(true, true), &[Configured, Agent, Default]);
        assert_eq!(ssh_steps(true, false), &[Configured, Default]);
        // 密钥格式不受支持（configured=false）：从 agent 开始，无 agent 时只试默认密钥
        assert_eq!(ssh_steps(false, true), &[Agent, Default]);
        assert_eq!(ssh_steps(false, false), &[Default]);
    }

    #[test]
    fn ssh_key_backend_supported_by_header() {
        let dir = std::env::temp_dir().join(format!("higit-keyfmt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let write = |name: &str, head: &str| {
            let p = dir.join(name);
            std::fs::write(&p, head).unwrap();
            p.to_string_lossy().into_owned()
        };
        // 三平台 libssh2 统一 OpenSSL 后端：仅 .ppk 无解析器 → 不进链路
        assert!(!ssh_key_backend_supported(&write(
            "putty.ppk",
            "PuTTY-User-Key-File-3: ssh-rsa\nEncryption: none\nComment: \n"
        )));
        // OpenSSH 容器（ed25519）/ PKCS#8 / EC / PEM RSA：引擎均可直接解析
        assert!(ssh_key_backend_supported(&write(
            "openssh.key",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc"
        )));
        assert!(ssh_key_backend_supported(&write(
            "pkcs8.key",
            "-----BEGIN PRIVATE KEY-----\nabc"
        )));
        assert!(ssh_key_backend_supported(&write(
            "ec.key",
            "-----BEGIN EC PRIVATE KEY-----\nabc"
        )));
        // PEM RSA / DSA：可用
        assert!(ssh_key_backend_supported(&write(
            "rsa.key",
            "-----BEGIN RSA PRIVATE KEY-----\nabc"
        )));
        // 文件不存在：交给凭证构造阶段正常失败并走链路下一步
        assert!(ssh_key_backend_supported("Z:/definitely/missing.key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auth_log_reports_attempt_reject_and_succeed() {
        let events: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
        {
            let sink = events.clone();
            let on_event = move |e: SyncEvent| {
                if let SyncEvent::Log { level, text } = e {
                    sink.lock().unwrap().push((level, text));
                }
            };
            let auth = AuthLog::new(&on_event);
            // 第一轮：配置的密钥被远端拒绝，链路步进到 ssh-agent
            auth.attempt("配置的密钥 D:/keys/id".into());
            auth.reject_last();
            // 第二轮：agent 也被拒绝，链路耗尽
            auth.attempt("ssh-agent".into());
            auth.reject_last();
            auth.log("error", "认证: 所有验证方式均被远端拒绝".into());
            // 新一轮（keyring 重试）：agent 通过
            auth.attempt("ssh-agent".into());
            auth.succeed();
            // succeed 后不再有未上报的"被拒绝"
            auth.reject_last();
        }
        let ev = events.lock().unwrap();
        let texts: Vec<&str> = ev.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "认证: 尝试 配置的密钥 D:/keys/id",
                "认证: 配置的密钥 D:/keys/id 被远端拒绝",
                "认证: 尝试 ssh-agent",
                "认证: ssh-agent 被远端拒绝",
                "认证: 所有验证方式均被远端拒绝",
                "认证: 尝试 ssh-agent",
                "认证通过: ssh-agent",
            ]
        );
        let levels: Vec<&str> = ev.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(levels, ["dim", "warn", "dim", "warn", "error", "dim", "ok"]);
    }
}
