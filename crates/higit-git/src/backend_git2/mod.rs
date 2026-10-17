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

use crate::engine::{
    CancelToken, ChangeFileInfo, CloneRequest, Credentials, FetchUpdate, GitEngine, Progress,
    PullOutcome, PushOutcome, PushPreview, PushRequest, RepoSession, SessionInner, SyncEvent,
};
use crate::error::{AuthDetail, AuthKind, ErrorCode, HigitError, Result};
use crate::model::{
    BranchInfo, CleanMode, CleanOptions, CleanOutcome, CommitDetail, CommitEntry, CommitFile,
    CommitFileKind, DiffLine, DiffLineOrigin, DiffSpec, FileEntry, FileStatusKind, HeadInfo,
    Identity, IdentityScope, MergeOutcome, RepoStatus, StatusOpts, TextDiff,
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
    fn pull_impl(
        &self,
        repo: &Repository,
        branch: &str,
        creds: Option<&Credentials>,
        progress: Option<&dyn Fn(Progress)>,
        on_event: Option<&(dyn Fn(SyncEvent) + Send + Sync)>,
        cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(HigitError::new(ErrorCode::Unknown, "no branch to pull"));
        }
        let local = repo
            .find_branch(branch, git2::BranchType::Local)
            .map_err(|_| {
                HigitError::new(ErrorCode::Unknown, format!("branch not found: {branch}"))
            })?;
        let is_current = local.is_head();
        let upstream = local
            .upstream()
            .map_err(|_| HigitError::new(ErrorCode::NoUpstream, branch.to_string()))?;
        let up_name = upstream
            .name()
            .ok()
            .flatten()
            .ok_or_else(|| HigitError::new(ErrorCode::NoUpstream, branch.to_string()))?;
        let (remote_name, remote_branch) = up_name
            .split_once('/')
            .ok_or_else(|| HigitError::new(ErrorCode::NoUpstream, up_name.to_string()))?;

        let log = |level: &str, text: String| {
            if let Some(ev) = on_event {
                ev(SyncEvent::Log {
                    level: level.to_string(),
                    text,
                });
            }
        };

        // 仅抓取当前分支的上游 refspec
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
        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(cbs);
        if let Some(px) = self.proxy_opts() {
            opts.proxy_options(px);
        }
        let spec =
            format!("+refs/heads/{remote_branch}:refs/remotes/{remote_name}/{remote_branch}");
        r.fetch(std::slice::from_ref(&spec), Some(&mut opts), None)
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
            return Ok(PullOutcome::default());
        };
        if local_oid == up_oid {
            return Ok(PullOutcome::default());
        }
        let (ahead, behind) = repo
            .graph_ahead_behind(local_oid, up_oid)
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
        if behind == 0 {
            // 本地领先或对等：无需更新
            return Ok(PullOutcome::default());
        }
        if ahead > 0 {
            return Err(HigitError::new(
                ErrorCode::Diverged,
                format!("{branch} vs {up_name}"),
            ));
        }

        // 快进：当前分支先安全 checkout（不覆盖未提交修改）→ 前移分支引用；
        // 非当前分支仅前移引用（无工作区变更）
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
        if is_current {
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
            .set_target(up_oid, "pull: fast-forward")
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

        Ok(PullOutcome {
            updated: true,
            head: Some(HeadInfo {
                short_id: up_oid.to_string().chars().take(7).collect(),
                summary: commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_string(),
                time_unix: commit.time().seconds(),
                committer_email: commit.committer().email().unwrap_or("").to_string(),
            }),
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

    /// 多分支推送（推送对话框）：refspecs 显式给定，支持 force；
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

            // 组装 refspec（空远端分支 = 同名）；force 等价前缀 "+"
            let mut refspecs = Vec::with_capacity(req.refspecs.len());
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
                let prefix = if req.force { "+" } else { "" };
                refspecs.push(format!("{prefix}refs/heads/{local}:refs/heads/{remote}"));
            }
            if refspecs.is_empty() {
                return Err(HigitError::new(ErrorCode::Unknown, "no branch to push"));
            }

            let remote_name = resolve_remote_name(repo, "", None)?;
            let mut r = repo
                .find_remote(&remote_name)
                .map_err(|e| HigitError::new(ErrorCode::NetworkFailed, e.message()))?;
            let url = r.url().unwrap_or_default().to_string();
            let auth = AuthLog::new(on_event);

            if cancel.is_cancelled() {
                return Err(HigitError::cancelled());
            }

            // git 命令回显（TortoiseGit 风格首行）
            let display = req
                .refspecs
                .iter()
                .map(|(l, rm)| {
                    let remote = if rm.trim().is_empty() {
                        l.as_str()
                    } else {
                        rm.trim()
                    };
                    format!("{}:{}", l.trim(), remote)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let force_flag = if req.force { " --force" } else { "" };
            log(
                "dim",
                format!("git push{force_flag} --progress \"{remote_name}\" {display}"),
            );
            log("dim", format!("远端: {url}"));

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
                    let short = refname.trim_start_matches("refs/heads/");
                    log("ok", format!("{short} -> {short}  ✓"));
                    Ok(())
                }
                Some(msg) => {
                    let short = refname.trim_start_matches("refs/heads/");
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

            // 逐分支建立缺失的上游（等价 -u；推送多分支时目标为各自的远端分支名）
            let mut created_upstream = false;
            for (local, remote) in &req.refspecs {
                let local = local.trim();
                let remote = if remote.trim().is_empty() {
                    local
                } else {
                    remote.trim()
                };
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
    /// 分支非当前检出时仅前移本地引用，不更新工作区。
    fn pull(
        &self,
        session: &RepoSession,
        branch: &str,
        creds: Option<&Credentials>,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<PullOutcome> {
        self.with_session(session, |_s, repo| {
            self.pull_impl(repo, branch, creds, Some(progress), None, cancel)
        })
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
        self.with_session(session, |_s, repo| {
            self.pull_impl(repo, branch, creds, None, Some(on_event), cancel)
        })
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
                });
            }
            Ok(files)
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

            let err = |e: git2::Error| HigitError::new(ErrorCode::Unknown, e.message());
            let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            // 勾选路径先按工作区内容暂存：只复制仓库索引条目的话，未暂存文件取到的是
            // HEAD 旧内容，拼出的树与 HEAD 相同，会产生空提交
            let rels: Vec<PathBuf> = paths.iter().map(|p| PathBuf::from(p.replace('\\', "/"))).collect();
            let mut repo_index = repo.index().map_err(err)?;
            for rel in &rels {
                if s.workdir.join(rel).is_file() {
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
            // 拼出的树与 HEAD 相同说明无变更可提交，拒绝（git 默认不产生空提交）
            if let Some(c) = &head {
                if tree_oid == c.tree_id() && repo.find_reference("MERGE_HEAD").is_err() {
                    return Err(HigitError::new(ErrorCode::Unknown, "nothing to commit"));
                }
            }
            let tree = repo.find_tree(tree_oid).map_err(err)?;

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
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
                .map_err(err)?;
            if merge_head.is_some() {
                let _ = repo.cleanup_state();
            }
            Ok(oid.to_string())
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
                if let Some(name) = branch.name().ok().flatten() {
                    let short = branch.get().target().map(short_id).unwrap_or_default();
                    out.push(BranchInfo {
                        name: name.to_string(),
                        is_remote: true,
                        is_current: false,
                        short_id: short,
                        last_commit_summary: String::new(),
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
    fn merge_branch(&self, session: &RepoSession, name: &str) -> Result<MergeOutcome> {
        self.with_session(session, |_s, repo| {
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

            let their_branch = repo
                .find_branch(name, BranchType::Local)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let their_oid = their_branch
                .get()
                .target()
                .ok_or_else(|| HigitError::new(ErrorCode::Unknown, "branch has no target"))?;
            let their_commit = repo
                .find_commit(their_oid)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let annotated = repo
                .find_annotated_commit(their_oid)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            let (analysis, _) = repo
                .merge_analysis(&[&annotated])
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            if analysis.is_up_to_date() {
                return Ok(MergeOutcome::default());
            }

            if analysis.is_fast_forward() {
                let mut opts = git2::build::CheckoutBuilder::new();
                repo.checkout_tree(their_commit.as_object(), Some(&mut opts))
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
                    .set_target(their_oid, &format!("merge: fast-forward {name}"))
                    .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
                return Ok(MergeOutcome {
                    fast_forward: true,
                    updated: true,
                    conflicts: Vec::new(),
                });
            }

            // 三方合并：干净结束 → 写合并提交；冲突 → 保留冲突状态并列出文件（FR-5.8 P0）
            let mut merge_opts = git2::MergeOptions::new();
            let mut checkout = git2::build::CheckoutBuilder::new();
            repo.merge(&[&annotated], Some(&mut merge_opts), Some(&mut checkout))
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            let mut index = repo
                .index()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            if index.has_conflicts() {
                return Ok(MergeOutcome {
                    fast_forward: false,
                    updated: false,
                    conflicts: list_conflicts(repo)?,
                });
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

            let config = repo
                .config()
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let name_cfg = config
                .get_string("user.name")
                .ok()
                .filter(|s| !s.is_empty());
            let email = config
                .get_string("user.email")
                .ok()
                .filter(|s| !s.is_empty());
            let (n, e) = match (name_cfg, email) {
                (Some(n), Some(e)) => (n, e),
                _ => return Err(HigitError::identity_missing()),
            };
            let sig = Signature::now(&n, &e)
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            let head_commit = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;
            let msg = format!("Merge branch '{name}'");
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &msg,
                &tree,
                &[&head_commit, &their_commit],
            )
            .map_err(|e| HigitError::new(ErrorCode::Unknown, e.message()))?;

            Ok(MergeOutcome {
                fast_forward: false,
                updated: true,
                conflicts: Vec::new(),
            })
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
    HigitError::new(ErrorCode::NetworkFailed, msg)
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
