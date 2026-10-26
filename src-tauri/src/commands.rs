//! Tauri 命令层：引擎（higit-git）与应用状态（config / session / jobs）的桥接。
//!
//! 所有命令均为 async + spawn_blocking：libgit2 是阻塞 API，不得卡渲染进程。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use higit_core::credentials as cred_store;
use higit_core::gitignore;
use higit_git::engine::{
    CancelToken, CloneRequest, Credentials, GitEngine, Progress, PullOptions, PushRequest,
    SyncEvent,
};
use higit_git::error::ErrorCode;
use higit_git::model::{DiffSpec, FileStatusKind, Identity, IdentityScope, StatusOpts};

use crate::dto::*;
use crate::error::{CmdResult, CommandError};
use crate::platform;
use crate::state::{AppState, SessionHandle, WatchHandle};

// ---------- 基础设施 ----------

pub async fn run_blocking<T, F>(f: F) -> CmdResult<T>
where
    F: FnOnce() -> CmdResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::msg("unknown", format!("join error: {e}")))?
}

fn no_repo() -> CommandError {
    CommandError::msg("no-repo", "no repository open")
}

fn with_session<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&AppState, &mut SessionHandle) -> CmdResult<T>,
) -> CmdResult<T> {
    let mut guard = state.session.lock().unwrap();
    let h = guard.as_mut().ok_or_else(no_repo)?;
    f(state, h)
}

/// 计算并缓存仓库状态（含忽略项，显示层过滤）。
pub fn compute_status(
    state: &AppState,
    h: &mut SessionHandle,
) -> CmdResult<higit_git::model::RepoStatus> {
    let s = state.engine.status(
        &h.session,
        &StatusOpts {
            include_ignored: true,
        },
    )?;
    h.last_status = Some(s.clone());
    Ok(s)
}

fn status_dto(state: &AppState, h: &mut SessionHandle) -> CmdResult<StatusDto> {
    compute_status(state, h).map(|s| StatusDto::from_model(&s))
}

// ---------- 事件负载 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEvent {
    pub path: String,
    pub status: StatusDto,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgressEvent {
    pub job_id: u32,
    pub stage: String,
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDoneEvent {
    pub job_id: u32,
    /// "clone" | "push" | "pull" | "fetch"
    pub kind: String,
    pub ok: bool,
    pub error: Option<CommandError>,
    /// clone 成功时携带目标仓库路径（前端据此打开仓库）
    pub repo_path: Option<String>,
    /// push: { createdUpstream } / pull: { updated, headShortId }
    pub outcome: Option<serde_json::Value>,
}

// ---------- 文件监听（FR-2.5）：300ms 防抖自动刷新 ----------

pub fn start_watcher(app: AppHandle, repo_path: PathBuf) -> Option<WatchHandle> {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |_res| {
        let _ = tx.send(());
    })
    .ok()?;
    watcher
        .watch(&repo_path, notify::RecursiveMode::Recursive)
        .ok()?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        loop {
            if stop2.load(Ordering::SeqCst) {
                return;
            }
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Err(_) => continue, // 空闲轮询，顺带检查 stop
                Ok(()) => {
                    // 防抖：事件静默 300ms 后才刷新
                    while let Ok(()) = rx.recv_timeout(std::time::Duration::from_millis(300)) {}
                    if stop2.load(Ordering::SeqCst) {
                        return;
                    }
                    watcher_refresh(&app, &repo_path);
                }
            }
        }
    });
    Some(WatchHandle {
        stop,
        _watcher: watcher,
    })
}

/// watcher 触发的状态刷新：静默推送给前端（跳过写任务进行中 / 非当前仓库）。
fn watcher_refresh(app: &AppHandle, repo: &Path) {
    let state = app.state::<AppState>();
    if !state.config.lock().unwrap().auto_refresh {
        return;
    }
    if state.write_busy.load(Ordering::SeqCst) {
        return;
    }
    let mut guard = state.session.lock().unwrap();
    let Some(h) = guard.as_mut() else { return };
    if h.path != repo {
        return;
    }
    if let Ok(s) = state.engine.status(
        &h.session,
        &StatusOpts {
            include_ignored: true,
        },
    ) {
        h.last_status = Some(s.clone());
        let _ = app.emit(
            "status-updated",
            StatusEvent {
                path: abs_string(repo),
                status: StatusDto::from_model(&s),
            },
        );
    }
}

// ---------- 启动 / 配置 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapDto {
    pub config: ConfigDto,
    pub system_theme: String,
    /// 系统语言（"zh-CN" / "en" …），language 为空（跟随系统）时前端使用
    pub system_language: String,
    pub version: String,
    pub config_corrupted: bool,
}

#[tauri::command]
pub async fn get_state(app: AppHandle) -> CmdResult<BootstrapDto> {
    let (config, corrupted) = crate::state::load_config();
    let proxy = config.proxy.clone();
    let proxy_on = config.proxy_enabled && !config.proxy.trim().is_empty();
    let engine = app.state::<AppState>().engine.clone();
    run_blocking(move || {
        engine.set_proxy(if proxy_on { Some(proxy.as_str()) } else { None });
        let sys_lang = sys_locale::get_locale().unwrap_or_else(|| "en".to_string());
        Ok(BootstrapDto {
            config: to_config_dto(&config),
            system_theme: if platform::os_prefers_dark() {
                "dark".into()
            } else {
                "light".into()
            },
            system_language: sys_lang,
            version: app.package_info().version.to_string(),
            config_corrupted: corrupted,
        })
    })
    .await
}

pub fn to_config_dto(cfg: &higit_core::config::AppConfig) -> ConfigDto {
    ConfigDto {
        language: cfg.language.clone(),
        theme: cfg.theme.clone(),
        recent_repos: cfg
            .recent_repos
            .iter()
            .map(|e| RepoEntryDto {
                id: e.id.clone(),
                name: e.name.clone(),
                path: abs_string(&e.path),
                alias: e.alias.clone(),
                auth: e.auth.as_ref().map(|a| AuthCfgDto {
                    kind: a.kind.clone(),
                    username: a.username.clone(),
                    key_path: a.key_path.clone(),
                }),
                url: e.url.clone(),
                identity: e.identity.as_ref().map(|i| IdentityDto {
                    name: i.name.clone(),
                    email: i.email.clone(),
                }),
            })
            .collect(),
        last_repo: abs_string(Path::new(&cfg.last_repo)),
        auto_refresh: cfg.auto_refresh,
        show_ignored: cfg.show_ignored,
        git_backend: cfg.git_backend.clone(),
        proxy_enabled: cfg.proxy_enabled,
        proxy: cfg.proxy.clone(),
    }
}

pub fn config_from_dto(dto: &ConfigDto) -> higit_core::config::AppConfig {
    higit_core::config::AppConfig {
        language: dto.language.clone(),
        theme: dto.theme.clone(),
        recent_repos: dto
            .recent_repos
            .iter()
            .map(|e| higit_core::config::RepoEntry {
                id: e.id.clone(),
                name: e.name.clone(),
                path: to_path(&e.path),
                alias: e.alias.clone(),
                auth: e.auth.as_ref().map(|a| higit_core::config::RepoAuth {
                    kind: a.kind.clone(),
                    username: a.username.clone(),
                    key_path: a.key_path.clone(),
                }),
                url: e.url.clone(),
                identity: e
                    .identity
                    .as_ref()
                    .map(|i| higit_core::config::RepoIdentity {
                        name: i.name.clone(),
                        email: i.email.clone(),
                    }),
            })
            .collect(),
        last_repo: to_path(&dto.last_repo).to_string_lossy().to_string(),
        auto_refresh: dto.auto_refresh,
        show_ignored: dto.show_ignored,
        git_backend: dto.git_backend.clone(),
        proxy_enabled: dto.proxy_enabled,
        proxy: dto.proxy.clone(),
    }
}

#[tauri::command]
pub async fn save_config(app: AppHandle, config: ConfigDto) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let mut cfg = config_from_dto(&config);
        // 仓库列表 / 最后打开仓库由专用命令（add/remove/reorder/open/close）维护；
        // 前端持有的列表可能是过期的，这里不能整体覆盖，否则会冲掉新增或复活已删除的仓库
        {
            let cur = state.config.lock().unwrap();
            cfg.recent_repos = cur.recent_repos.clone();
            cfg.last_repo = cur.last_repo.clone();
        }
        state
            .engine
            .set_proxy(if cfg.proxy_enabled && !cfg.proxy.trim().is_empty() {
                Some(cfg.proxy.as_str())
            } else {
                None
            });
        *state.config.lock().unwrap() = cfg;
        state.save_config();
        Ok(())
    })
    .await
}

// ---------- 仓库管理（FR-1） ----------

#[tauri::command]
pub async fn list_repos(app: AppHandle) -> CmdResult<Vec<RepoSummaryDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let entries = state.config.lock().unwrap().recent_repos.clone();
        let mut out = Vec::new();
        for e in entries {
            let (branch, dirty) = match state.engine.open(&e.path) {
                Ok(session) => {
                    let st = state.engine.status(
                        &session,
                        &StatusOpts {
                            include_ignored: true,
                        },
                    );
                    match st {
                        Ok(s) => (
                            s.branch,
                            s.entries
                                .iter()
                                .filter(|f| {
                                    !matches!(
                                        f.kind,
                                        FileStatusKind::Clean | FileStatusKind::Ignored
                                    )
                                })
                                .count() as u32,
                        ),
                        Err(_) => (None, 0),
                    }
                }
                Err(_) => (None, 0),
            };
            out.push(RepoSummaryDto {
                id: e.id.clone(),
                name: e.name,
                path: abs_string(&e.path),
                alias: e.alias.clone(),
                branch,
                dirty,
            });
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub async fn add_local_repo(app: AppHandle, path: String) -> CmdResult<RepoEntryDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let p = to_path(&path);
        // 校验是否为有效 Git 仓库（FR-1.4）
        let session = state.engine.open(&p)?;
        // 读取仓库自带参数入库：origin 远端 URL + 仓库级 Git 身份（有则保存，无则跟随默认）
        let url = state
            .engine
            .remote_url(&session, "origin")?
            .unwrap_or_default();
        let identity =
            state
                .engine
                .local_identity(&session)?
                .map(|i| higit_core::config::RepoIdentity {
                    name: i.name,
                    email: i.email,
                });
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string_lossy().to_string());
        let entry = higit_core::config::RepoEntry {
            id: String::new(),
            name,
            path: p,
            alias: None,
            auth: None,
            url,
            identity,
        };
        let mut cfg = state.config.lock().unwrap();
        cfg.upsert_recent_repo(entry.clone());
        drop(cfg);
        state.save_config();
        Ok(RepoEntryDto {
            id: entry.id.clone(),
            name: entry.name.clone(),
            path: abs_string(&entry.path),
            alias: entry.alias.clone(),
            auth: None,
            url: entry.url.clone(),
            identity: entry.identity.as_ref().map(|i| IdentityDto {
                name: i.name.clone(),
                email: i.email.clone(),
            }),
        })
    })
    .await
}

#[tauri::command]
pub async fn remove_repo(app: AppHandle, path: String) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let p = to_path(&path);
        {
            let mut cfg = state.config.lock().unwrap();
            cfg.recent_repos.retain(|e| e.path != p);
            if cfg.last_repo == p.to_string_lossy() {
                cfg.last_repo.clear();
            }
        }
        state.save_config();
        Ok(())
    })
    .await
}

/// 仓库列表拖拽排序（FR-1.1）：按前端传来的路径顺序重排并持久化。
#[tauri::command]
pub async fn reorder_repos(app: AppHandle, paths: Vec<String>) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let mut cfg = state.config.lock().unwrap();
        // 与 remove_repo 一致：先转本地分隔符再按 path 匹配
        let keys: Vec<PathBuf> = paths.iter().map(|p| to_path(p)).collect();
        let mut reordered = Vec::with_capacity(cfg.recent_repos.len());
        for key in &keys {
            if let Some(pos) = cfg.recent_repos.iter().position(|e| &e.path == key) {
                reordered.push(cfg.recent_repos.remove(pos));
            }
        }
        // 未出现在请求里的条目（理论不存在）保持相对顺序排在末尾
        reordered.append(&mut cfg.recent_repos);
        cfg.recent_repos = reordered;
        drop(cfg);
        state.save_config();
        Ok(())
    })
    .await
}

/// 打开仓库：建立会话、启动 watcher、恢复 last_repo。
#[tauri::command]
pub async fn open_repo(app: AppHandle, path: String) -> CmdResult<RepoViewDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let p = to_path(&path);
        let session = state.engine.open(&p)?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string_lossy().to_string());

        // 关闭旧会话（停止旧 watcher）
        if let Some(old) = state.session.lock().unwrap().take() {
            old.stop_watch();
        }

        let mut h = SessionHandle {
            session,
            path: p.clone(),
            last_status: None,
            watch: None,
        };
        let status = compute_status(&state, &mut h)?;
        h.watch = start_watcher(app.clone(), p.clone());

        *state.session.lock().unwrap() = Some(h);

        {
            let mut cfg = state.config.lock().unwrap();
            cfg.last_repo = p.to_string_lossy().to_string();
        }
        state.save_config();

        Ok(RepoViewDto {
            path: abs_string(&p),
            name,
            status: StatusDto::from_model(&status),
        })
    })
    .await
}

#[tauri::command]
pub async fn close_repo(app: AppHandle) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        if let Some(old) = state.session.lock().unwrap().take() {
            old.stop_watch();
        }
        {
            let mut cfg = state.config.lock().unwrap();
            cfg.last_repo.clear();
        }
        state.save_config();
        Ok(())
    })
    .await
}

/// 目标目录检查："missing" | "empty" | "nonempty"
#[tauri::command]
pub async fn dest_status(path: String) -> CmdResult<String> {
    run_blocking(move || {
        let p = to_path(&path);
        Ok(if !p.exists() {
            "missing".to_string()
        } else if p
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            "empty".to_string()
        } else {
            "nonempty".to_string()
        })
    })
    .await
}

/// 仓库设置（仓库设置对话框 / 克隆对话框的认证与身份入库，FR-1.7 / FR-5.5）。
/// 机密字段不落盘：密码 / Token / 密钥口令始终走系统凭证管理器。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSettingsReq {
    /// 仓库唯一 ID（优先）；克隆等尚未取得 id 的场景留空，回退按 path 定位
    #[serde(default)]
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthCfgDto>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub identity: Option<IdentityDto>,
    /// HTTPS 密码 / Token（机密，不落 config.json）：Some(非空) 时写入系统凭证管理器
    #[serde(default)]
    pub password: Option<String>,
    /// 是否记住凭证到系统凭证管理器；None = true。
    /// false 且密码留空时清除已存凭证
    #[serde(default)]
    pub remember_cred: Option<bool>,
}

/// 读取单个仓库条目（仓库设置对话框数据源）：按 id 实时取自后端 config，
/// 避免前端持有的列表快照过期。
#[tauri::command]
pub async fn get_repo_settings(app: AppHandle, id: String) -> CmdResult<RepoEntryDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().unwrap();
        let e = cfg
            .recent_repos
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CommandError::msg("unknown", "repository not in list"))?;
        Ok(RepoEntryDto {
            id: e.id.clone(),
            name: e.name.clone(),
            path: abs_string(&e.path),
            alias: e.alias.clone(),
            auth: e.auth.as_ref().map(|a| AuthCfgDto {
                kind: a.kind.clone(),
                username: a.username.clone(),
                key_path: a.key_path.clone(),
            }),
            url: e.url.clone(),
            identity: e.identity.as_ref().map(|i| IdentityDto {
                name: i.name.clone(),
                email: i.email.clone(),
            }),
        })
    })
    .await
}

#[tauri::command]
pub async fn save_repo_settings(app: AppHandle, req: RepoSettingsReq) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let p = to_path(&req.path);
        {
            let mut cfg = state.config.lock().unwrap();
            // id 优先（同名仓库也各自唯一）；id 为空时回退按路径定位
            let located = if req.id.is_empty() {
                cfg.recent_repos.iter_mut().find(|e| e.path == p)
            } else {
                cfg.recent_repos.iter_mut().find(|e| e.id == req.id)
            };
            let Some(entry) = located else {
                return Err(CommandError::msg("unknown", "repository not in list"));
            };
            entry.alias = req.alias.clone();
            entry.auth = req.auth.as_ref().map(|a| higit_core::config::RepoAuth {
                kind: a.kind.clone(),
                username: a.username.clone(),
                key_path: a.key_path.clone(),
            });
            entry.url = req.url.clone();
            entry.identity = req
                .identity
                .as_ref()
                .map(|i| higit_core::config::RepoIdentity {
                    name: i.name.clone(),
                    email: i.email.clone(),
                });
        }
        state.save_config();

        // HTTPS 凭证入库（机密只进系统凭证管理器，不落 config.json）。
        // host 优先取仓库设置的远端 URL，留空时回退仓库 git 配置的 origin——
        // 与同步时的凭证预取同一解析规则（此前前端只认 URL 覆盖字段，留空即漏存）。
        if let Some(auth) = &req.auth {
            if auth.kind == "http" {
                let remember = req.remember_cred.unwrap_or(true);
                let password = req.password.as_deref().unwrap_or("");
                let host = repo_https_url(&state, &p, &req.url).and_then(|u| https_host(&u));
                if let Some(host) = host {
                    match (remember, password.is_empty()) {
                        (true, false) => cred_store::store(
                            &higit_core::credentials::StoredCredential {
                                kind: "http".into(),
                                username: auth.username.clone(),
                                secret: password.to_string(),
                                key_path: String::new(),
                            },
                            &host,
                        )
                        .map_err(|e| {
                            CommandError::msg("unknown", format!("save credential: {e}"))
                        })?,
                        // 取消"记住"且未输入新密码：视为清除已存凭证
                        (false, true) => {
                            let _ = cred_store::forget(&host, &auth.username);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })
    .await
}

/// 新建仓库（FR-1.1）：init + 可选 README / .gitignore 模板 + 初始提交（身份缺失优雅降级）。
#[tauri::command]
pub async fn new_repo(
    app: AppHandle,
    parent_dir: String,
    name: String,
    with_readme: bool,
    with_gitignore: bool,
    gitignore_stack: String,
) -> CmdResult<RepoViewDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let dest = to_path(&parent_dir).join(&name);
        let session = state.engine.init(&dest)?;
        if with_readme {
            let _ = std::fs::write(dest.join("README.md"), format!("# {name}\n"));
        }
        if with_gitignore {
            let _ = std::fs::write(
                dest.join(".gitignore"),
                gitignore::template(&gitignore_stack),
            );
        }
        // 初始提交；身份未配置则跳过（保留未跟踪状态，后续引导设置）
        let _ = state.engine.commit(&session, "Initial commit", None);

        if let Some(old) = state.session.lock().unwrap().take() {
            old.stop_watch();
        }
        let mut h = SessionHandle {
            session,
            path: dest.clone(),
            last_status: None,
            watch: None,
        };
        let status = compute_status(&state, &mut h)?;
        h.watch = start_watcher(app.clone(), dest.clone());
        *state.session.lock().unwrap() = Some(h);

        {
            let mut cfg = state.config.lock().unwrap();
            cfg.upsert_recent_repo(higit_core::config::RepoEntry {
                id: String::new(),
                name: name.clone(),
                path: dest.clone(),
                alias: None,
                auth: None,
                url: String::new(),
                identity: None,
            });
            cfg.last_repo = dest.to_string_lossy().to_string();
        }
        state.save_config();

        Ok(RepoViewDto {
            path: abs_string(&dest),
            name,
            status: StatusDto::from_model(&status),
        })
    })
    .await
}

// ---------- 工作区浏览（FR-2） ----------

#[tauri::command]
pub async fn refresh_status(app: AppHandle) -> CmdResult<StatusDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, status_dto)
    })
    .await
}

/// list_dir 行状态判定（FR-2.2/2.4）：
/// - 条目命中自身 → 直接采用；
/// - 未命中 → 沿祖先查折叠目录条目：status 对整目录未跟踪/忽略只报一条 "dir/"，
///   目录内部条目继承该状态，否则进入目录后全部误显示"未修改"；
/// - 目录 → 任一非忽略后代变更 → Modified（目录聚合）；
/// - 其余 → Clean。忽略状态透传给前端（有"忽略"样式），show_ignored=false 由调用方过滤。
fn row_status_kind(
    map: &HashMap<String, FileStatusKind>,
    dir_changed: &HashSet<String>,
    rel_path: &str,
    is_dir: bool,
) -> FileStatusKind {
    let kind = map.get(rel_path).copied().or_else(|| {
        let mut acc = String::new();
        let mut inherited = None;
        for seg in rel_path.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            if let Some(k) = map.get(&acc) {
                if matches!(k, FileStatusKind::Untracked | FileStatusKind::Ignored) {
                    inherited = Some(*k);
                }
            }
        }
        inherited
    });
    match kind {
        Some(k) if k != FileStatusKind::Clean => k,
        _ if is_dir && dir_changed.contains(rel_path) => FileStatusKind::Modified,
        _ => FileStatusKind::Clean,
    }
}

/// 文件列表：文件系统目录 + Git 状态聚合（目录聚合后代变更，FR-2.2/2.4）。
/// show_ignored 由前端显式传入：保存配置与刷新列表是并发调用，读后端配置会慢一拍产生竞态。
#[tauri::command]
pub async fn list_dir(
    app: AppHandle,
    rel: String,
    show_ignored: Option<bool>,
) -> CmdResult<Vec<FileRowDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let show_ignored =
            show_ignored.unwrap_or_else(|| state.config.lock().unwrap().show_ignored);
        with_session(&state, |_state, h| {
            let mut map: HashMap<String, FileStatusKind> = HashMap::new();
            let mut dir_changed: HashSet<String> = HashSet::new();
            if let Some(status) = &h.last_status {
                for e in &status.entries {
                    let p = rel_string(&e.path);
                    map.insert(p.clone(), e.kind);
                    if !matches!(e.kind, FileStatusKind::Ignored) {
                        // 目录聚合：任一非忽略后代变更 → 目录标"已修改"
                        let mut parts: Vec<&str> = p.split('/').collect();
                        parts.pop();
                        let mut acc = String::new();
                        for seg in parts {
                            if !acc.is_empty() {
                                acc.push('/');
                            }
                            acc.push_str(seg);
                            dir_changed.insert(acc.clone());
                        }
                    }
                }
            }

            let dir = if rel.is_empty() {
                h.path.clone()
            } else {
                h.path.join(to_path(&rel))
            };
            let read =
                std::fs::read_dir(&dir).map_err(|e| CommandError::msg("unknown", e.to_string()))?;
            let mut dirs: Vec<FileRowDto> = Vec::new();
            let mut files: Vec<FileRowDto> = Vec::new();
            for entry in read.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let rel_path = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let effective = row_status_kind(&map, &dir_changed, &rel_path, is_dir);
                if !show_ignored && effective == FileStatusKind::Ignored {
                    continue;
                }
                let mtime_unix = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                let meta = entry.metadata().ok();
                let size = if is_dir {
                    None
                } else {
                    meta.as_ref().map(|m| m.len())
                };
                let row = FileRowDto {
                    name,
                    rel_path,
                    abs_path: abs_string(&path),
                    is_dir,
                    kind: kind_str(effective),
                    size,
                    mtime_unix,
                };
                if is_dir {
                    dirs.push(row);
                } else {
                    files.push(row);
                }
            }
            let by_name =
                |a: &FileRowDto, b: &FileRowDto| a.name.to_lowercase().cmp(&b.name.to_lowercase());
            dirs.sort_by(by_name);
            files.sort_by(by_name);
            dirs.extend(files);
            Ok(dirs)
        })
    })
    .await
}

/// 目录树（懒加载展开，目录行上限 4000；仅目录，文件在右侧列表查看）。
#[tauri::command]
pub async fn list_tree(app: AppHandle, expanded: Vec<String>) -> CmdResult<Vec<TreeRowDto>> {
    run_blocking(move || {
        const CAP: usize = 4000;
        let state = app.state::<AppState>();
        let set: HashSet<String> = expanded.into_iter().collect();
        with_session(&state, |_state, h| {
            let root = h.path.clone();
            let mut rows = Vec::new();
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.to_string_lossy().to_string());
            rows.push(TreeRowDto {
                abs_path: abs_string(&root),
                name,
                level: 0,
                git_meta: false,
                has_children: has_subdir(&root),
            });
            // 根目录同样遵循展开状态：收起时只返回根行（前端 openRepo 默认展开根）
            if set.contains(&abs_string(&root)) {
                walk_tree(&root, &root, &set, 1, &mut rows, CAP);
            }
            Ok(rows)
        })
    })
    .await
}

/// 目录下是否还有子目录（叶子目录不显示展开箭头 / 换实心图标区分）
fn has_subdir(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    read.flatten()
        .any(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
}

fn walk_tree(
    repo_root: &Path,
    dir: &Path,
    expanded: &HashSet<String>,
    level: u32,
    rows: &mut Vec<TreeRowDto>,
    cap: usize,
) {
    if rows.len() >= cap {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(PathBuf, String)> = read
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| (e.path(), e.file_name().to_string_lossy().to_string()))
        .collect();
    entries.sort_by_key(|a| a.1.to_lowercase());
    for (path, name) in entries {
        if rows.len() >= cap {
            return;
        }
        let git_meta = dir == repo_root && name == ".git";
        // .git 元目录恒折叠（与原型一致）
        let is_expanded = !git_meta && expanded.contains(&abs_string(&path));
        rows.push(TreeRowDto {
            abs_path: abs_string(&path),
            name,
            level,
            git_meta,
            has_children: has_subdir(&path),
        });
        if is_expanded {
            walk_tree(repo_root, &path, expanded, level + 1, rows, cap);
        }
    }
}

// ---------- 暂存 / 丢弃 / 提交（FR-3 / FR-4） ----------

#[tauri::command]
pub async fn stage_paths(app: AppHandle, paths: Vec<String>) -> CmdResult<StatusDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let conv: Vec<PathBuf> = paths.iter().map(|p| to_path(p)).collect();
            state.engine.stage(&h.session, &conv)?;
            status_dto(state, h)
        })
    })
    .await
}

#[tauri::command]
pub async fn unstage_paths(app: AppHandle, paths: Vec<String>) -> CmdResult<StatusDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let conv: Vec<PathBuf> = paths.iter().map(|p| to_path(p)).collect();
            state.engine.unstage(&h.session, &conv)?;
            status_dto(state, h)
        })
    })
    .await
}

#[tauri::command]
pub async fn discard_paths(app: AppHandle, paths: Vec<String>) -> CmdResult<StatusDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let conv: Vec<PathBuf> = paths.iter().map(|p| to_path(p)).collect();
            state.engine.discard(&h.session, &conv)?;
            status_dto(state, h)
        })
    })
    .await
}

/// 工作区清理（git clean 语义）：按清理对话框选项删除未跟踪 / 被忽略条目
#[tauri::command]
pub async fn clean_worktree(app: AppHandle, opts: CleanOptsDto) -> CmdResult<CleanOutcomeDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let o = state.engine.clean_worktree(&h.session, &opts.to_model())?;
            Ok(CleanOutcomeDto::from_model(&o))
        })
    })
    .await
}

/// 提交身份：仓库设置（config 条目）优先；否则后端回退 git 配置（仓库级 → 全局）。
fn commit_identity(state: &AppState, repo: &Path) -> Option<Identity> {
    let cfg = state.config.lock().unwrap();
    cfg.recent_repos
        .iter()
        .find(|e| e.path == repo)
        .and_then(|e| e.identity.as_ref())
        .filter(|i| !i.name.trim().is_empty() && !i.email.trim().is_empty())
        .map(|i| Identity {
            name: i.name.trim().to_string(),
            email: i.email.trim().to_string(),
        })
}

#[tauri::command]
pub async fn commit(
    app: AppHandle,
    message: String,
    amend: bool,
    // 按路径提交（提交对话框勾选语义）：仅提交这些路径（None/空 = 提交整个暂存区）
    paths: Option<Vec<String>>,
    // 提交到新分支（提交对话框勾选）：在 HEAD 处建分支并切换后提交，原分支不动
    new_branch: Option<String>,
) -> CmdResult<String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let author = commit_identity(state, &h.path);
            // 新分支：与 HEAD 同树，仅移动 HEAD 符号引用，无需检出文件
            if let Some(nb) = new_branch
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                state.engine.create_branch(&h.session, nb, None)?;
                state.engine.switch_head(&h.session, nb)?;
            }
            let oid = if amend {
                if let Some(paths) = paths.filter(|p| !p.is_empty()) {
                    state.engine.commit_amend_partial(
                        &h.session,
                        &message,
                        author.as_ref(),
                        &paths,
                    )?
                } else {
                    state
                        .engine
                        .commit_amend(&h.session, &message, author.as_ref())?
                }
            } else if let Some(paths) = paths.filter(|p| !p.is_empty()) {
                state
                    .engine
                    .commit_partial(&h.session, &message, author.as_ref(), &paths)?
            } else {
                state.engine.commit(&h.session, &message, author.as_ref())?
            };
            Ok(oid.chars().take(7).collect())
        })
    })
    .await
}

// ---------- 提交对话框（FR-4 扩展）：全量变更预览 + 勾选提交 ----------

/// 提交对话框的变更文件行。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileDto {
    /// 相对仓库根路径（正斜杠）
    pub path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "conflicted"
    pub kind: String,
    /// 该路径当前是否已有暂存变更（未勾选时提交前需反暂存）
    pub staged: bool,
    /// 磁盘文件大小（字节；已删除 / 不存在为 null）
    pub size: Option<u64>,
    /// 磁盘最后修改时间（Unix 秒；已删除为 null）
    pub mtime_unix: Option<i64>,
    /// 相对 HEAD 的新增行数（二进制 / 文件过多未统计为 null）
    pub additions: Option<u32>,
    /// 相对 HEAD 的删除行数
    pub deletions: Option<u32>,
}

/// 提交对话框数据：仓库信息 + 提交者 + 变更文件列表。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPreviewDto {
    pub name: String,
    pub path: String,
    /// "name <email>"；未配置身份时为空串
    pub committer: String,
    pub files: Vec<CommitFileDto>,
    /// 因超过 Windows MAX_PATH(260) 无法列入清单的文件数（> 0 时 UI 提示可启用长路径支持）
    pub long_path_count: u32,
    /// 超长路径样例（最多 3 条，供提示悬停展示）
    pub long_path_samples: Vec<String>,
}

/// 提交对话框预览：仓库名 + 提交者 + `rel` 目录下的全部变更文件（状态 / 大小 / 修改时间）。
#[tauri::command]
pub async fn commit_preview(app: AppHandle, rel: String) -> CmdResult<CommitPreviewDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let mut preview = state.engine.commit_preview(&h.session, &rel)?;
            // 行级变更统计（+行 / -行）；文件过多时引擎侧整体返回 None
            let paths: Vec<String> = preview.files.iter().map(|f| f.path.clone()).collect();
            let stats = state.engine.change_stats(&h.session, &paths)?;
            for (f, (a, d)) in preview.files.iter_mut().zip(stats) {
                f.additions = a;
                f.deletions = d;
            }
            let files = preview
                .files
                .into_iter()
                .map(|f| CommitFileDto {
                    path: f.path,
                    kind: f.kind,
                    staged: f.staged,
                    size: f.size,
                    mtime_unix: f.mtime_unix,
                    additions: f.additions,
                    deletions: f.deletions,
                })
                .collect();
            // 提交者展示链与提交命令一致：仓库设置 → git 配置（仓库级 → 全局）
            let identity = commit_identity(state, &h.path).or_else(|| {
                state
                    .engine
                    .identity(&h.session)
                    .ok()
                    .flatten()
                    .or_else(|| state.engine.global_identity().ok().flatten())
            });
            Ok(CommitPreviewDto {
                name: h
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: abs_string(&h.path),
                committer: identity
                    .map(|i| format!("{} <{}>", i.name, i.email))
                    .unwrap_or_default(),
                files,
                long_path_count: preview.long_path_count,
                long_path_samples: preview.long_path_samples,
            })
        })
    })
    .await
}

/// 启用当前仓库的长路径支持（写本地 core.longpaths = true，提交对话框的
/// 显式入口）。写入后需重新打开仓库会话（open_repo）让 libgit2 按新配置
/// 枚举 / 暂存，前端随后刷新提交预览。
#[tauri::command]
pub async fn enable_core_longpaths(app: AppHandle) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            Ok(state.engine.enable_core_longpaths(&h.session)?)
        })
    })
    .await
}

/// HEAD 提交消息（Amend 预填，FR-4.5）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMessageDto {
    pub summary: String,
    pub body: String,
}

#[tauri::command]
pub async fn last_commit_message(app: AppHandle) -> CmdResult<Option<CommitMessageDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let entries = state.engine.log(&h.session, 1, 0)?;
            let Some(first) = entries.first().cloned() else {
                return Ok(None);
            };
            let detail = state.engine.commit_detail(&h.session, &first.id)?;
            let mut parts = detail.message.splitn(2, '\n');
            let summary = parts.next().unwrap_or("").to_string();
            let body = parts
                .next()
                .unwrap_or("")
                .trim_start_matches('\r')
                .to_string();
            Ok(Some(CommitMessageDto { summary, body }))
        })
    })
    .await
}

// ---------- 差异（FR-8） ----------

#[tauri::command]
pub async fn diff_file(app: AppHandle, path: String, spec: String) -> CmdResult<DiffDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let spec = if spec == "staged" {
                DiffSpec::Staged
            } else {
                DiffSpec::Workdir
            };
            let td = state.engine.diff_text(&h.session, &to_path(&path), spec)?;
            Ok(DiffDto::from_model(&td))
        })
    })
    .await
}

#[tauri::command]
pub async fn commit_file_diff(app: AppHandle, oid: String, path: String) -> CmdResult<DiffDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let td = state
                .engine
                .commit_file_diff(&h.session, &oid, &to_path(&path))?;
            Ok(DiffDto::from_model(&td))
        })
    })
    .await
}

// ---------- 同步长任务（FR-5）：clone / push / pull / fetch ----------

/// 引擎凭证 → keyring 存储。
fn creds_to_stored(c: &Credentials) -> higit_core::credentials::StoredCredential {
    match c {
        Credentials::UserPass { username, password } => higit_core::credentials::StoredCredential {
            kind: "http".into(),
            username: username.clone(),
            secret: password.clone(),
            key_path: String::new(),
        },
        Credentials::SshKey {
            username,
            key_path,
            passphrase,
            ..
        } => higit_core::credentials::StoredCredential {
            kind: "ssh".into(),
            username: username.clone(),
            secret: passphrase.clone(),
            key_path: key_path.clone(),
        },
    }
}

/// keyring 存储 → 引擎凭证（按认证上下文补全）。
fn stored_to_creds(
    stored: &higit_core::credentials::StoredCredential,
    host: &str,
    username_hint: &str,
) -> Credentials {
    let _ = host;
    let username = if stored.username.is_empty() {
        username_hint.to_string()
    } else {
        stored.username.clone()
    };
    if stored.kind == "ssh" {
        Credentials::SshKey {
            username: if username.is_empty() {
                "git".into()
            } else {
                username
            },
            key_path: stored.key_path.clone(),
            passphrase: stored.secret.clone(),
            from_store: true,
        }
    } else {
        Credentials::UserPass {
            username,
            password: stored.secret.clone(),
        }
    }
}

/// 认证重试链：显式凭证失败（或未提供）时尝试 keyring 中的"记住"凭证，仅一次；
/// 已存凭证失效则清除
fn retry_with_stored(
    used: &Option<Credentials>,
    e: &higit_git::error::HigitError,
    mut run: impl FnMut(Option<&Credentials>) -> Result<(), higit_git::error::HigitError>,
) -> Result<(), higit_git::error::HigitError> {
    if e.code != ErrorCode::AuthFailed {
        return Err(e.clone());
    }
    let Some(auth) = &e.auth else {
        return Err(e.clone());
    };
    let Some(stored) = cred_store::load(&auth.host, &auth.username) else {
        return Err(e.clone());
    };
    let retry = stored_to_creds(&stored, &auth.host, &auth.username);
    if used.as_ref() == Some(&retry) {
        return Err(e.clone()); // 与本次已用凭证相同，重试必然同败
    }
    match run(Some(&retry)) {
        Ok(()) => Ok(()),
        Err(e2) if e2.code == ErrorCode::AuthFailed => {
            let _ = cred_store::forget(&auth.host, &stored.username);
            Err(e2)
        }
        Err(e2) => Err(e2),
    }
}

fn progress_cb(app: AppHandle, job_id: u32) -> impl Fn(Progress) + Send + Sync + 'static {
    move |p: Progress| {
        let _ = app.emit(
            "job-progress",
            JobProgressEvent {
                job_id,
                stage: p.stage.to_string(),
                done: p.done,
                total: p.total,
            },
        );
    }
}

/// 从 http(s) URL 提取主机名（宽松解析：去 scheme / userinfo / 端口）。
fn https_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// 仓库生效的远端 URL：仓库设置的 URL 覆盖 → 仓库 git 配置的 origin。
/// 仅 http(s) URL 有效（SSH 远端的凭证走 ssh_creds_from_entry），否则 None。
fn repo_https_url(state: &AppState, repo: &Path, url_override: &str) -> Option<String> {
    let url = if url_override.trim().is_empty() {
        let session = state.engine.open(repo).ok()?;
        state.engine.remote_url(&session, "origin").ok()??
    } else {
        url_override.trim().to_string()
    };
    (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
}

/// 仓库设置"记住"的 HTTPS 凭证（keyring 预取）：命中后首次同步即携带，
/// 不必先发一次无凭证请求、等 401 后再走 keyring 重试。
fn http_creds_from_store(state: &AppState, repo: &Path) -> Option<Credentials> {
    let (username_hint, url_override) = {
        let cfg = state.config.lock().unwrap();
        let e = cfg.recent_repos.iter().find(|e| e.path == repo)?;
        (
            e.auth
                .as_ref()
                .filter(|a| a.kind == "http")
                .map(|a| a.username.clone())
                .unwrap_or_default(),
            e.url.clone(),
        )
    };
    let url = repo_https_url(state, repo, &url_override)?;
    let host = https_host(&url)?;
    let stored = cred_store::load(&host, &username_hint)?;
    (stored.kind == "http").then(|| stored_to_creds(&stored, &host, &username_hint))
}

/// 仓库是否已在系统凭证管理器保存 HTTPS 凭证（设置对话框回显"已保存"提示用）。
#[tauri::command]
pub async fn repo_credential_saved(app: AppHandle, id: String) -> CmdResult<bool> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let (repo_path, username, url_override) = {
            let cfg = state.config.lock().unwrap();
            let e = cfg
                .recent_repos
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| CommandError::msg("unknown", "repository not in list"))?;
            (
                e.path.clone(),
                e.auth
                    .as_ref()
                    .map(|a| a.username.clone())
                    .unwrap_or_default(),
                e.url.clone(),
            )
        };
        let Some(url) = repo_https_url(&state, &repo_path, &url_override) else {
            return Ok(false);
        };
        let Some(host) = https_host(&url) else {
            return Ok(false);
        };
        Ok(cred_store::load(&host, &username).is_some())
    })
    .await
}

/// 仓库设置中的 SSH 凭证（仅 SSH；HTTP 口令属机密不落盘，始终走 keyring / 凭证对话框）。
fn ssh_creds_from_entry(state: &AppState, repo: &Path) -> Option<Credentials> {
    let cfg = state.config.lock().unwrap();
    cfg.recent_repos
        .iter()
        .find(|e| e.path == repo)
        .and_then(|e| e.auth.as_ref())
        .and_then(|auth| {
            if auth.kind != "ssh" || (auth.key_path.is_empty() && auth.username.trim().is_empty()) {
                return None;
            }
            Some(Credentials::SshKey {
                username: if auth.username.trim().is_empty() {
                    "git".to_string()
                } else {
                    auth.username.trim().to_string()
                },
                key_path: auth.key_path.clone(),
                passphrase: String::new(),
                from_store: false,
            })
        })
}

/// 仓库设置的目标远端 URL：与 origin 配置不一致时先对齐再同步。
fn apply_url_override(state: &AppState, repo: &Path, session: &higit_git::engine::RepoSession) {
    let url = {
        let cfg = state.config.lock().unwrap();
        cfg.recent_repos
            .iter()
            .find(|e| e.path == repo)
            .map(|e| e.url.trim().to_string())
            .unwrap_or_default()
    };
    if url.is_empty() {
        return;
    }
    if let Ok(Some(current)) = state.engine.remote_url(session, "origin") {
        if current != url {
            let _ = state.engine.set_remote_url(session, "origin", &url);
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReq {
    /// "push" | "pull" | "fetch"
    pub kind: String,
    /// 推送 / 拉取的目标本地分支；抓取忽略
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub creds: Option<CredsDto>,
    #[serde(default)]
    pub remember: bool,
}

#[tauri::command]
pub async fn sync(app: AppHandle, req: SyncReq) -> CmdResult<u32> {
    let kind: &'static str = match req.kind.as_str() {
        "push" => "push",
        "pull" => "pull",
        "fetch" => "fetch",
        _ => return Err(CommandError::msg("unknown", "bad sync kind")),
    };
    let explicit = req.creds.as_ref().map(|c| c.clone().into_engine());
    let remember = req.remember;
    let branch = req.branch.clone();
    let (job_id, cancel) = app.state::<AppState>().register_job(kind);

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        state.write_busy.store(true, Ordering::SeqCst);
        let progress = progress_cb(app.clone(), job_id);
        // 凭证对话框提交且勾选"记住"：由前端调用 save_credential 写入系统凭证管理器
        let _ = remember;
        let outcome = sync_once(
            &app,
            &state.engine,
            kind,
            &branch,
            explicit.as_ref(),
            &progress,
            &cancel,
        );
        state.write_busy.store(false, Ordering::SeqCst);
        state.finish_job(job_id);

        let payload = match &outcome {
            Ok(o) => Some(o.clone()),
            Err(_) => None,
        };
        let error = outcome.err().map(CommandError::from);
        let _ = app.emit(
            "job-done",
            JobDoneEvent {
                job_id,
                kind: kind.to_string(),
                ok: error.is_none(),
                error,
                repo_path: None,
                outcome: payload,
            },
        );
    });

    Ok(job_id)
}

/// 单次同步尝试（含 keyring 重试链与远端 URL 对齐）。
fn sync_once(
    app: &AppHandle,
    engine: &Arc<dyn GitEngine>,
    kind: &str,
    branch: &str,
    explicit: Option<&Credentials>,
    progress: &dyn Fn(Progress),
    cancel: &CancelToken,
) -> Result<serde_json::Value, higit_git::error::HigitError> {
    let state = app.state::<AppState>();
    let guard = state.session.lock().unwrap();
    let Some(h) = guard.as_ref() else {
        return Err(higit_git::error::HigitError::new(
            ErrorCode::Unknown,
            "no repo",
        ));
    };
    apply_url_override(&state, &h.path, &h.session);
    let used = explicit
        .cloned()
        .or_else(|| ssh_creds_from_entry(&state, &h.path))
        .or_else(|| http_creds_from_store(&state, &h.path));
    drop(guard);

    let run =
        |creds: Option<&Credentials>| -> Result<serde_json::Value, higit_git::error::HigitError> {
            let guard = state.session.lock().unwrap();
            let Some(h) = guard.as_ref() else {
                return Err(higit_git::error::HigitError::new(
                    ErrorCode::Unknown,
                    "no repo",
                ));
            };
            match kind {
                "push" => {
                    let o = engine.push(&h.session, branch, creds, progress, cancel)?;
                    Ok(serde_json::json!({ "createdUpstream": o.created_upstream }))
                }
                "pull" => {
                    let o = engine.pull(&h.session, branch, creds, progress, cancel)?;
                    Ok(serde_json::json!({
                        "updated": o.updated,
                        "headShortId": o.head.map(|h| h.short_id).unwrap_or_default(),
                    }))
                }
                _ => {
                    engine.fetch(&h.session, "", creds, progress, cancel)?;
                    Ok(serde_json::json!(null))
                }
            }
        };

    let result = run(used.as_ref());
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            // 认证失败：先试 keyring 中的"记住"凭证，仅一次
            match retry_with_stored(&used, &e, |c| run(c).map(|_| ())) {
                Ok(()) => Ok(serde_json::json!(null)),
                Err(e2) => Err(e2),
            }
        }
    }
}

#[tauri::command]
pub async fn cancel_job(state: State<'_, AppState>, job_id: u32) -> CmdResult<()> {
    if let Some(job) = state.jobs.lock().unwrap().get(&job_id) {
        job.cancel.cancel();
    }
    Ok(())
}

// ---------- 同步对话框（FR-5.2 扩展）：分支预览 + 多分支推送详细日志 ----------

/// 同步对话框的本地分支行。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBranchDto {
    pub name: String,
    pub is_current: bool,
    pub short_id: String,
    pub summary: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// 同步对话框数据：仓库信息 + 远端 + 分支列表。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPreviewDto {
    pub name: String,
    pub path: String,
    pub remote_name: String,
    pub url: String,
    /// 配置的所有远端（推送对话框的目标远端选择）
    pub remotes: Vec<SyncRemoteDto>,
    pub branches: Vec<SyncBranchDto>,
    pub remote_branches: Vec<String>,
}

/// 同步对话框的远端选项（名称 + URL）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRemoteDto {
    pub name: String,
    pub url: String,
}

#[tauri::command]
pub async fn sync_preview(app: AppHandle) -> CmdResult<SyncPreviewDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let (remote_name, remote_url) =
                state.engine.sync_remote(&h.session)?.unwrap_or_default();
            // 仓库设置保存的 URL 优先展示（与同步时的 URL 覆盖行为一致）
            let url = {
                let cfg = state.config.lock().unwrap();
                cfg.recent_repos
                    .iter()
                    .find(|e| e.path == h.path)
                    .map(|e| e.url.trim().to_string())
                    .filter(|s| !s.is_empty())
            }
            .unwrap_or(remote_url);
            let all = state.engine.branches(&h.session)?;
            let branches = all
                .iter()
                .filter(|b| !b.is_remote)
                .map(|b| SyncBranchDto {
                    name: b.name.clone(),
                    is_current: b.is_current,
                    short_id: b.short_id.clone(),
                    summary: b.last_commit_summary.clone(),
                    upstream: b.upstream.clone(),
                    ahead: b.ahead,
                    behind: b.behind,
                })
                .collect();
            let remote_prefix = format!("{remote_name}/");
            let remote_branches = all
                .iter()
                .filter(|b| b.is_remote && b.name.starts_with(&remote_prefix))
                .map(|b| b.name[remote_prefix.len()..].to_string())
                .collect();
            let remotes = state
                .engine
                .remotes(&h.session)?
                .into_iter()
                .map(|(name, url)| SyncRemoteDto { name, url })
                .collect();
            Ok(SyncPreviewDto {
                name: h
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: abs_string(&h.path),
                remote_name,
                url,
                remotes,
                branches,
                remote_branches,
            })
        })
    })
    .await
}

/// 同步日志事件（同步对话框日志区）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLogEvent {
    pub job_id: u32,
    /// "info" | "ok" | "warn" | "error" | "remote" | "dim"
    pub level: String,
    pub text: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRefspecReq {
    pub local: String,
    #[serde(default)]
    pub remote: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStartReq {
    /// "push" | "pull" | "fetch"
    pub kind: String,
    /// push：多分支 refspec（空远端分支 = 同名）
    #[serde(default)]
    pub refspecs: Vec<PushRefspecReq>,
    /// pull：目标本地分支
    #[serde(default)]
    pub branch: String,
    /// push：强制推送
    #[serde(default)]
    pub force: bool,
    /// push：目标远端名（空 = 自动解析）
    #[serde(default)]
    pub push_remote: String,
    /// push：任意 URL 推送（优先于 push_remote）
    #[serde(default)]
    pub push_url: String,
    /// push：安全强制（--force-with-lease，与 force 互斥）
    #[serde(default)]
    pub force_with_lease: bool,
    /// push：同时推送所有本地标签（--tags）
    #[serde(default)]
    pub push_tags: bool,
    /// push：无上游时建立跟踪（缺省 true 保持既有行为）
    #[serde(default = "default_true")]
    pub set_upstream: bool,
    #[serde(default)]
    pub creds: Option<CredsDto>,
    #[serde(default)]
    pub remember: bool,
    /// pull：TortoiseGit 式拉取选项（缺省 = 快进优先、分叉报错、不 prune）
    #[serde(default)]
    pub pull_opts: Option<PullOptsDto>,
}

fn default_true() -> bool {
    true
}

/// 拉取选项（FR-5.1 扩展，拉取对话框提交）
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullOptsDto {
    #[serde(default)]
    pub rebase: bool,
    #[serde(default)]
    pub no_ff: bool,
    #[serde(default)]
    pub ff_only: bool,
    #[serde(default)]
    pub no_commit: bool,
    #[serde(default)]
    pub squash: bool,
    #[serde(default)]
    pub all_tags: bool,
    #[serde(default)]
    pub prune: bool,
}

impl PullOptsDto {
    pub fn to_model(&self) -> PullOptions {
        PullOptions {
            rebase: self.rebase,
            no_ff: self.no_ff,
            ff_only: self.ff_only,
            no_commit: self.no_commit,
            squash: self.squash,
            all_tags: self.all_tags,
            prune: self.prune,
        }
    }
}

// ---------- 出网方式 / 认证方式 / 阶段说明（同步与克隆日志共用） ----------

/// 出网方式说明。HiGit 网络操作的代理只来自自身设置（FR-10.4）：开启时 libgit2 以
/// GIT_PROXY_SPECIFIED 走该代理；未开启时为 GIT_PROXY_NONE 直连，此时 git 配置的
/// http.proxy 与 http_proxy / HTTPS_PROXY 等环境变量都不参与（libgit2 仅在 AUTO
/// 模式下读取它们）；SSH 传输由 libssh2 建立 TCP 连接，不经过代理。
fn egress_note(url: &str, proxy: Option<&str>, zh: bool) -> String {
    let is_ssh = url.starts_with("git@") || url.starts_with("ssh://");
    let is_local = !is_ssh && !url.starts_with("http://") && !url.starts_with("https://");
    if is_local {
        return if zh {
            "出网方式: 本地路径，无网络访问".to_string()
        } else {
            "Network: local path, no network access".to_string()
        };
    }
    match proxy {
        Some(p) if !p.trim().is_empty() && !is_ssh => {
            if zh {
                format!("出网方式: 走代理 {p}")
            } else {
                format!("Network: via proxy {p}")
            }
        }
        _ => {
            if zh {
                "出网方式: 直连".to_string()
            } else {
                "Network: direct connection".to_string()
            }
        }
    }
}

/// 生效凭证链的认证方式说明（同步与克隆日志共用，文案与原同步对话框一致）。
fn auth_note(used: Option<&Credentials>, zh: bool) -> String {
    match used {
        Some(Credentials::SshKey {
            username, key_path, ..
        }) => {
            if key_path.is_empty() {
                if zh {
                    "认证方式: SSH（ssh-agent / 默认密钥）".to_string()
                } else {
                    "Auth: SSH (ssh-agent / default keys)".to_string()
                }
            } else if zh {
                format!("认证方式: SSH 密钥 {key_path}（用户 {username}）")
            } else {
                format!("Auth: SSH key {key_path} (user {username})")
            }
        }
        Some(Credentials::UserPass { username, .. }) => {
            if zh {
                format!("认证方式: HTTPS（用户 {username}）")
            } else {
                format!("Auth: HTTPS (user {username})")
            }
        }
        None => {
            if zh {
                "认证方式: 默认（ssh-agent / 默认密钥 / git 凭证链）".to_string()
            } else {
                "Auth: default (ssh-agent / default keys / git credential chain)".to_string()
            }
        }
    }
}

/// 进度阶段切换时的动作说明（同步与克隆日志共用；未知阶段原样输出）。
fn stage_note(stage: &str, zh: bool) -> String {
    match stage {
        "Packing objects" => {
            if zh {
                "正在打包对象…".to_string()
            } else {
                "Packing objects…".to_string()
            }
        }
        "Pushing objects" => {
            if zh {
                "正在推送对象…".to_string()
            } else {
                "Pushing objects…".to_string()
            }
        }
        "Receiving objects" => {
            if zh {
                "正在接收对象…".to_string()
            } else {
                "Receiving objects…".to_string()
            }
        }
        "Resolving deltas" => {
            if zh {
                "正在解析增量…".to_string()
            } else {
                "Resolving deltas…".to_string()
            }
        }
        _ => stage.to_string(),
    }
}

/// 当前 HiGit 设置中生效的网络代理（FR-10.4）：None = 未启用（直连）。
fn proxy_in_use(state: &AppState) -> Option<String> {
    let cfg = state.config.lock().unwrap();
    (cfg.proxy_enabled && !cfg.proxy.trim().is_empty()).then(|| cfg.proxy.trim().to_string())
}

/// SOCKS 代理警示（仅 socks*:// 地址返回 Some）。实测依据：Windows 上代理经系统
/// WinHTTP 处理，`socks5://` 地址与常见代理软件的 SOCKS 服务不兼容（稳定握手超时，
/// 见 examples/proxy_probe.rs）；libgit2 自带客户端（非 Windows）也没有 SOCKS 实现。
/// 引擎真正稳定可用的是代理软件的 HTTP 入口。
fn socks_proxy_hint(proxy: Option<&str>, zh: bool) -> Option<String> {
    let p = proxy?;
    if !p.trim().to_ascii_lowercase().starts_with("socks") {
        return None;
    }
    Some(if zh {
        "注意: SOCKS 代理与当前引擎兼容性差（常表现为超时），建议改用代理软件的 HTTP 代理入口（http://…）"
            .to_string()
    } else {
        "Note: SOCKS proxies are unreliable with this engine (often timeouts); prefer an HTTP proxy address (http://...)".to_string()
    })
}

/// 同步长任务（推送 / 拉取 / 抓取对话框）：与 sync 命令同链路（keyring 重试 / 取消），
/// 额外产出详细日志（目标地址 / 认证方式 / 待传输内容 / 远端输出 / 逐分支结果）。
#[tauri::command]
pub async fn sync_start(app: AppHandle, req: SyncStartReq) -> CmdResult<u32> {
    let kind: &'static str = match req.kind.as_str() {
        "push" => "push",
        "pull" => "pull",
        "fetch" => "fetch",
        _ => return Err(CommandError::msg("unknown", "bad sync kind")),
    };
    let refspecs: Vec<(String, String)> = req
        .refspecs
        .iter()
        .map(|r| (r.local.trim().to_string(), r.remote.trim().to_string()))
        .filter(|(l, _)| !l.is_empty())
        .collect();
    if kind == "push" && refspecs.is_empty() {
        return Err(CommandError::msg("unknown", "no branch to push"));
    }
    if kind == "pull" && req.branch.trim().is_empty() {
        return Err(CommandError::msg("unknown", "no branch to pull"));
    }
    let branch = req.branch.trim().to_string();
    let force = req.force;
    let explicit = req.creds.as_ref().map(|c| c.clone().into_engine());
    // "记住凭证"由前端在发起前调用 save_credential 写入系统凭证管理器
    let _remember = req.remember;
    let (job_id, cancel) = app.state::<AppState>().register_job(kind);

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        state.write_busy.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let zh = state.config.lock().unwrap().language == "zh-CN";
        let lz = move |zh_text: &str, en_text: &str| -> String {
            if zh {
                zh_text.to_string()
            } else {
                en_text.to_string()
            }
        };
        let app_log = app.clone();
        let emit_log = move |level: &str, text: String| {
            let _ = app_log.emit(
                "sync-log",
                SyncLogEvent {
                    job_id,
                    level: level.to_string(),
                    text,
                },
            );
        };

        let repo_path = state
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|h| h.path.clone());
        let finish = |ok: bool, error: Option<CommandError>, outcome: Option<serde_json::Value>| {
            state.write_busy.store(false, Ordering::SeqCst);
            state.finish_job(job_id);
            let _ = app.emit(
                "job-done",
                JobDoneEvent {
                    job_id,
                    kind: kind.into(),
                    ok,
                    error,
                    repo_path: None,
                    outcome,
                },
            );
        };
        let Some(repo_path) = repo_path else {
            emit_log("error", lz("没有打开的仓库", "No repository open"));
            finish(
                false,
                Some(CommandError::msg("no-repo", "no repository open")),
                None,
            );
            return;
        };

        // 远端 URL 对齐（与快速同步一致：仓库设置 URL 覆盖 origin）
        {
            let guard = state.session.lock().unwrap();
            if let Some(h) = guard.as_ref() {
                apply_url_override(&state, &h.path, &h.session);
            }
        }

        // 生效凭证链：对话框凭证 → 仓库设置 SSH → keyring 预取（失败时重试一次）
        let used = explicit
            .clone()
            .or_else(|| ssh_creds_from_entry(&state, &repo_path))
            .or_else(|| http_creds_from_store(&state, &repo_path));
        // 出网方式（开始前输出）：直连 / 走代理（HiGit 网络代理设置，FR-10.4）
        let remote_url = {
            let guard = state.session.lock().unwrap();
            match guard.as_ref() {
                Some(h) => state
                    .engine
                    .sync_remote(&h.session)
                    .ok()
                    .flatten()
                    .map(|(_, u)| u),
                None => None,
            }
        };
        let proxy_cfg = proxy_in_use(&state);
        emit_log(
            "info",
            egress_note(
                remote_url.as_deref().unwrap_or(""),
                proxy_cfg.as_deref(),
                zh,
            ),
        );
        if let Some(h) = socks_proxy_hint(proxy_cfg.as_deref(), zh) {
            emit_log("warn", h);
        }
        emit_log("info", auth_note(used.as_ref(), zh));

        // 各操作的预览日志
        match kind {
            "push" => {
                for (local, remote) in &refspecs {
                    let remote = if remote.is_empty() {
                        local.clone()
                    } else {
                        remote.clone()
                    };
                    let pv = {
                        let guard = state.session.lock().unwrap();
                        match guard.as_ref() {
                            Some(h) => state.engine.push_preview(&h.session, local, &remote).ok(),
                            None => None,
                        }
                    };
                    match pv {
                        Some(p) if p.commits == 0 => emit_log(
                            "dim",
                            lz(
                                &format!("分支 {local} → {remote}：远端已是最新"),
                                &format!("Branch {local} -> {remote}: up to date"),
                            ),
                        ),
                        Some(p) => {
                            emit_log(
                                "info",
                                lz(
                                    &format!(
                                        "分支 {local} → {remote}：{} 个提交，{} 个文件",
                                        p.commits,
                                        p.files.len()
                                    ),
                                    &format!(
                                        "Branch {local} -> {remote}: {} commits, {} files",
                                        p.commits,
                                        p.files.len()
                                    ),
                                ),
                            );
                            const FILE_CAP: usize = 50;
                            for f in p.files.iter().take(FILE_CAP) {
                                emit_log("dim", format!("  {f}"));
                            }
                            if p.files.len() > FILE_CAP {
                                emit_log(
                                    "dim",
                                    lz(
                                        &format!("  …（共 {} 个文件）", p.files.len()),
                                        &format!("  … ({} files in total)", p.files.len()),
                                    ),
                                );
                            }
                        }
                        None => emit_log(
                            "warn",
                            lz(
                                &format!("分支 {local}：无法计算推送预览（分支不存在？）"),
                                &format!(
                                    "Branch {local}: cannot compute preview (missing branch?)"
                                ),
                            ),
                        ),
                    }
                }
            }
            "pull" => {
                let info = {
                    let guard = state.session.lock().unwrap();
                    match guard.as_ref() {
                        Some(h) => state.engine.branches(&h.session).ok(),
                        None => None,
                    }
                };
                if let Some(b) = info
                    .and_then(|list| list.into_iter().find(|b| !b.is_remote && b.name == branch))
                {
                    match (&b.upstream, b.behind) {
                        (Some(u), n) if n > 0 => emit_log(
                            "info",
                            lz(
                                &format!("分支 {branch}：落后 {n} 个提交（{u}）"),
                                &format!("Branch {branch}: {n} commits behind ({u})"),
                            ),
                        ),
                        (Some(u), _) => emit_log(
                            "dim",
                            lz(
                                &format!("分支 {branch}：与 {u} 一致"),
                                &format!("Branch {branch}: up to date with {u}"),
                            ),
                        ),
                        (None, _) => emit_log(
                            "warn",
                            lz(
                                &format!("分支 {branch}：未设置上游分支"),
                                &format!("Branch {branch}: no upstream configured"),
                            ),
                        ),
                    }
                }
            }
            _ => emit_log(
                "info",
                lz(
                    "抓取远端所有分支的更新",
                    "Fetching all branches from the remote",
                ),
            ),
        }

        // 引擎事件 → 日志 / 进度；阶段切换时补一行动作说明
        let last_stage = std::sync::Mutex::new(String::new());
        let log_for_events = emit_log.clone();
        let app_for_events = app.clone();
        let on_event = move |ev: SyncEvent| match ev {
            SyncEvent::Log { level, text } => log_for_events(&level, text),
            SyncEvent::Progress { stage, done, total } => {
                if let Ok(mut ls) = last_stage.lock() {
                    if *ls != stage {
                        *ls = stage.to_string();
                        log_for_events("info", stage_note(stage, zh));
                    }
                }
                let _ = app_for_events.emit(
                    "job-progress",
                    JobProgressEvent {
                        job_id,
                        stage: stage.to_string(),
                        done,
                        total,
                    },
                );
            }
        };

        let run = |creds: Option<&Credentials>| -> Result<serde_json::Value, higit_git::error::HigitError> {
            let guard = state.session.lock().unwrap();
            let Some(h) = guard.as_ref() else {
                return Err(higit_git::error::HigitError::new(
                    ErrorCode::Unknown,
                    "no repo",
                ));
            };
            match kind {
                "push" => {
                    let preq = PushRequest {
                        refspecs: refspecs.clone(),
                        force,
                        remote: req.push_remote.clone(),
                        remote_url: req.push_url.clone(),
                        force_with_lease: req.force_with_lease,
                        tags: req.push_tags,
                        set_upstream: req.set_upstream,
                    };
                    state
                        .engine
                        .push_multi(&h.session, &preq, creds, &on_event, &cancel)?;
                    Ok(serde_json::json!({}))
                }
                "pull" => {
                    let o = state.engine.pull_options(
                        &h.session,
                        &branch,
                        &req.pull_opts
                            .as_ref()
                            .map(|p| p.to_model())
                            .unwrap_or_default(),
                        creds,
                        None,
                        Some(&on_event),
                        &cancel,
                    )?;
                    Ok(serde_json::json!({
                        "updated": o.updated,
                        "headShortId": o.head.map(|x| x.short_id).unwrap_or_default(),
                        "fastForward": o.fast_forward,
                        "rebased": o.rebased,
                        "pendingCommit": o.pending_commit,
                        "conflicts": o.conflicts.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect::<Vec<_>>(),
                    }))
                }
                _ => {
                    let updates =
                        state
                            .engine
                            .fetch_verbose(&h.session, "", creds, &on_event, &cancel)?;
                    Ok(serde_json::Value::Array(
                        updates
                            .iter()
                            .map(|u| {
                                serde_json::json!({
                                    "name": u.name,
                                    "oldShort": u.old_short,
                                    "newShort": u.new_short,
                                })
                            })
                            .collect(),
                    ))
                }
            }
        };

        let result = run(used.as_ref());
        let result = match result {
            Ok(v) => Ok(v),
            Err(e) => match retry_with_stored(&used, &e, |c| {
                emit_log(
                    "info",
                    lz("使用记住的凭证重试…", "Retrying with stored credentials…"),
                );
                run(c).map(|_| ())
            }) {
                Ok(()) => Ok(serde_json::json!({})),
                Err(e2) => Err(e2),
            },
        };

        let elapsed = started.elapsed().as_millis();
        match &result {
            Ok(v) => {
                match kind {
                    "pull" => {
                        let updated = v.get("updated").and_then(|x| x.as_bool()).unwrap_or(false);
                        let short = v.get("headShortId").and_then(|x| x.as_str()).unwrap_or("");
                        let rebased = v.get("rebased").and_then(|x| x.as_u64()).unwrap_or(0);
                        let ff = v
                            .get("fastForward")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                        let pending = v
                            .get("pendingCommit")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                        let conflicts: Vec<String> = v
                            .get("conflicts")
                            .and_then(|x| x.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !conflicts.is_empty() {
                            emit_log(
                                "warn",
                                lz(
                                    &format!(
                                        "合并被冲突阻断（{} 个文件），解决后在变更页提交",
                                        conflicts.len()
                                    ),
                                    &format!(
                                        "Merge blocked by conflicts ({} file(s)); resolve and commit",
                                        conflicts.len()
                                    ),
                                ),
                            );
                        } else if pending {
                            emit_log(
                                "ok",
                                lz(
                                    "合并已就绪但未自动提交，请检查后提交",
                                    "Merge is ready but not committed; commit manually",
                                ),
                            );
                        } else if rebased > 0 {
                            emit_log(
                                "ok",
                                lz(
                                    &format!(
                                        "已变基 {rebased} 个提交 → {short}（耗时 {elapsed} ms）"
                                    ),
                                    &format!(
                                        "Rebased {rebased} commit(s) -> {short} ({elapsed} ms)"
                                    ),
                                ),
                            );
                        } else if updated && !short.is_empty() {
                            emit_log(
                                "ok",
                                lz(
                                    &format!(
                                        "{} {short}（耗时 {elapsed} ms）",
                                        if ff { "已快进" } else { "已更新到" }
                                    ),
                                    &format!(
                                        "{} {short} ({elapsed} ms)",
                                        if ff {
                                            "Fast-forwarded to"
                                        } else {
                                            "Updated to"
                                        }
                                    ),
                                ),
                            );
                        } else {
                            emit_log(
                                "ok",
                                lz(
                                    &format!("已是最新（耗时 {elapsed} ms）"),
                                    &format!("Up to date ({elapsed} ms)"),
                                ),
                            );
                        }
                    }
                    "fetch" => {
                        if let Some(list) = v.as_array() {
                            if list.is_empty() {
                                emit_log("dim", lz("远端无更新", "Remote unchanged"));
                            }
                            for u in list {
                                let name = u.get("name").and_then(|x| x.as_str()).unwrap_or("");
                                let old = u.get("oldShort").and_then(|x| x.as_str()).unwrap_or("");
                                let new = u.get("newShort").and_then(|x| x.as_str()).unwrap_or("");
                                if old.is_empty() {
                                    emit_log(
                                        "ok",
                                        lz(
                                            &format!("新分支 {name}: {new}"),
                                            &format!("New branch {name}: {new}"),
                                        ),
                                    );
                                } else {
                                    emit_log(
                                        "ok",
                                        lz(
                                            &format!("{name}: {old} → {new}"),
                                            &format!("{name}: {old} -> {new}"),
                                        ),
                                    );
                                }
                            }
                        }
                        emit_log(
                            "ok",
                            lz(
                                &format!("成功（耗时 {elapsed} ms）"),
                                &format!("Success ({elapsed} ms)"),
                            ),
                        );
                    }
                    _ => emit_log(
                        "ok",
                        lz(
                            &format!("成功（耗时 {elapsed} ms）"),
                            &format!("Success ({elapsed} ms)"),
                        ),
                    ),
                }
                finish(true, None, Some(v.clone()));
            }
            Err(e) => {
                if e.code == ErrorCode::Cancelled {
                    emit_log(
                        "warn",
                        lz(
                            &format!("已取消（{elapsed} ms）"),
                            &format!("Cancelled ({elapsed} ms)"),
                        ),
                    );
                } else {
                    let raw = e.raw.clone().unwrap_or_default();
                    emit_log(
                        "error",
                        lz(
                            &format!("失败（{elapsed} ms）：{raw}"),
                            &format!("Failed ({elapsed} ms): {raw}"),
                        ),
                    );
                }
                finish(false, Some(CommandError::from(e.clone())), None);
            }
        }
    });

    Ok(job_id)
}

/// 凭证对话框"记住"：写入系统凭证管理器（FR-5.5）。
#[tauri::command]
pub async fn save_credential(host: String, creds: CredsDto) -> CmdResult<()> {
    run_blocking(move || {
        let c = creds.into_engine();
        let _ = cred_store::store(&creds_to_stored(&c), &host);
        Ok(())
    })
    .await
}

/// 克隆长任务（FR-1.2）：进度 + 取消 + keyring 重试链，经 `sync-log` 事件输出
/// 与同步对话框一致的详细日志（目标 / 认证 / 出网 / 阶段切换 / 结果 / 耗时）；
/// 成功后前端收到 repoPath，再调用 open_repo / save_repo_settings 入库认证与身份。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneReq {
    pub url: String,
    pub dest: String,
    #[serde(default)]
    pub creds: Option<CredsDto>,
    #[serde(default)]
    pub remember: bool,
}

#[tauri::command]
pub async fn clone_repo(app: AppHandle, req: CloneReq) -> CmdResult<u32> {
    let (job_id, cancel) = app.state::<AppState>().register_job("clone");
    let url = req.url.clone();
    let dest = to_path(&req.dest);
    let explicit = req.creds.as_ref().map(|c| c.clone().into_engine());
    let remember = req.remember;

    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        state.write_busy.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let zh = state.config.lock().unwrap().language == "zh-CN";
        let app_log = app.clone();
        let emit_log = move |level: &str, text: String| {
            let _ = app_log.emit(
                "sync-log",
                SyncLogEvent {
                    job_id,
                    level: level.to_string(),
                    text,
                },
            );
        };
        // 进度事件照旧驱动进度条；阶段切换时补一行动作说明
        let app_prog = app.clone();
        let log_for_progress = emit_log.clone();
        let last_stage = std::sync::Mutex::new(String::new());
        let progress = move |p: Progress| {
            if let Ok(mut ls) = last_stage.lock() {
                if *ls != p.stage {
                    *ls = p.stage.to_string();
                    log_for_progress("info", stage_note(p.stage, zh));
                }
            }
            let _ = app_prog.emit(
                "job-progress",
                JobProgressEvent {
                    job_id,
                    stage: p.stage.to_string(),
                    done: p.done,
                    total: p.total,
                },
            );
        };
        let engine = state.engine.clone();
        let creq = CloneRequest {
            url: url.clone(),
            dest: dest.clone(),
            depth: None,
        };

        // 开始信息：目标 / 出网 / 认证（与同步对话框开头一致）
        emit_log("dim", format!("git clone {url}"));
        emit_log(
            "dim",
            if zh {
                format!("目标目录: {}", dest.display())
            } else {
                format!("Destination: {}", dest.display())
            },
        );
        let proxy_cfg = proxy_in_use(&state);
        emit_log("info", egress_note(&url, proxy_cfg.as_deref(), zh));
        if let Some(h) = socks_proxy_hint(proxy_cfg.as_deref(), zh) {
            emit_log("warn", h);
        }
        emit_log("info", auth_note(explicit.as_ref(), zh));

        let mut result = engine.clone_repo(&creq, explicit.as_ref(), &progress, &cancel);
        if explicit.is_none() {
            if let Err(e) = &result {
                match retry_with_stored(&None, e, |c| {
                    emit_log(
                        "info",
                        if zh {
                            "使用记住的凭证重试…".to_string()
                        } else {
                            "Retrying with stored credentials…".to_string()
                        },
                    );
                    engine.clone_repo(&creq, c, &progress, &cancel).map(|_| ())
                }) {
                    Ok(()) => {
                        finish_clone(&app, job_id, dest, true, None);
                        return;
                    }
                    Err(e2) => result = Err(e2),
                }
            }
        }
        if remember {
            if let Some(creds) = &explicit {
                store_creds_for(&url, creds);
            }
        }
        let elapsed = started.elapsed().as_millis();
        match &result {
            Ok(_) => emit_log(
                "ok",
                if zh {
                    format!("成功（耗时 {elapsed} ms）")
                } else {
                    format!("Success ({elapsed} ms)")
                },
            ),
            Err(e) if e.code == ErrorCode::Cancelled => emit_log(
                "warn",
                if zh {
                    format!("已取消（{elapsed} ms）")
                } else {
                    format!("Cancelled ({elapsed} ms)")
                },
            ),
            Err(e) => {
                let raw = e.raw.clone().unwrap_or_default();
                emit_log(
                    "error",
                    if zh {
                        format!("失败（{elapsed} ms）：{raw}")
                    } else {
                        format!("Failed ({elapsed} ms): {raw}")
                    },
                );
            }
        }
        let (ok, error) = match &result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(CommandError::from(e.clone()))),
        };
        finish_clone(&app, job_id, dest, ok, error);
    });

    Ok(job_id)
}

// remember=true 时把凭证写入系统凭证管理器（按 URL host）
fn store_creds_for(url: &str, creds: &Credentials) {
    if let Some(host) = host_from_url(url) {
        let _ = cred_store::store(&creds_to_stored(creds), &host);
    }
}

/// 从 URL 提取 host（支持 https:// 与 git@host:path 两种形式；本地路径返回 None）。
fn host_from_url(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some(host) = rest.split(':').next() {
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
    }
    if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        let authority = rest.split('/').next().unwrap_or(rest);
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let host = authority.split(':').next().unwrap_or(authority);
        if !host.is_empty() {
            return Some(host.to_string());
        }
    }
    None
}

fn finish_clone(
    app: &AppHandle,
    job_id: u32,
    dest: PathBuf,
    ok: bool,
    error: Option<CommandError>,
) {
    let state = app.state::<AppState>();
    state.write_busy.store(false, Ordering::SeqCst);
    state.finish_job(job_id);
    let _ = app.emit(
        "job-done",
        JobDoneEvent {
            job_id,
            kind: "clone".into(),
            ok,
            error,
            repo_path: ok.then(|| abs_string(&dest)),
            outcome: None,
        },
    );
}

// ---------- 提交历史（FR-6） ----------

#[tauri::command]
pub async fn history_page(app: AppHandle, limit: u32, skip: u32) -> CmdResult<Vec<CommitEntryDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let list = state.engine.log(&h.session, limit, skip)?;
            Ok(list.iter().map(CommitEntryDto::from_model).collect())
        })
    })
    .await
}

#[tauri::command]
pub async fn commit_detail(app: AppHandle, oid: String) -> CmdResult<CommitDetailDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let d = state.engine.commit_detail(&h.session, &oid)?;
            Ok(CommitDetailDto::from_model(&d))
        })
    })
    .await
}

// ---------- 分支管理（FR-7） ----------

#[tauri::command]
pub async fn list_branches(app: AppHandle) -> CmdResult<Vec<BranchDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let list = state.engine.branches(&h.session)?;
            Ok(list.iter().map(BranchDto::from_model).collect())
        })
    })
    .await
}

#[tauri::command]
pub async fn branch_create(
    app: AppHandle,
    name: String,
    from: String,
    checkout: bool,
) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let from = if from.trim().is_empty() {
                None
            } else {
                Some(from.as_str())
            };
            state.engine.create_branch(&h.session, &name, from)?;
            if checkout {
                state.engine.checkout_branch(&h.session, &name)?;
                let _ = status_dto(state, h);
            }
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn branch_checkout(app: AppHandle, name: String) -> CmdResult<StatusDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            state.engine.checkout_branch(&h.session, &name)?;
            status_dto(state, h)
        })
    })
    .await
}

#[tauri::command]
pub async fn list_tags(app: AppHandle) -> CmdResult<Vec<TagDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            let list = state.engine.tags(&h.session)?;
            Ok(list.iter().map(TagDto::from_model).collect())
        })
    })
    .await
}

/// 增强检出（FR-7.5）：分支/标签/提交 + TortoiseGit 式选项（切换分支对话框提交）
#[tauri::command]
pub async fn checkout_target(
    app: AppHandle,
    opts: CheckoutOptsDto,
) -> CmdResult<CheckoutOutcomeDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let o = state.engine.checkout(&h.session, &opts.to_model())?;
            let _ = status_dto(state, h);
            Ok(CheckoutOutcomeDto::from_model(&o))
        })
    })
    .await
}

#[tauri::command]
pub async fn branch_rename(app: AppHandle, old: String, new: String) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            state.engine.rename_branch(&h.session, &old, &new)?;
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn branch_delete(app: AppHandle, name: String, force: bool) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            state.engine.delete_branch(&h.session, &name, force)?;
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn branch_merge(app: AppHandle, name: String) -> CmdResult<MergeOutcomeDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let o = state.engine.merge_branch(&h.session, &name)?;
            let _ = status_dto(state, h);
            Ok(MergeOutcomeDto::from_model(&o))
        })
    })
    .await
}

/// 增强合并（FR-7.6）：分支/标签/提交来源 + TortoiseGit 式选项（合并分支对话框提交）
#[tauri::command]
pub async fn merge_target(app: AppHandle, opts: MergeOptsDto) -> CmdResult<MergeResultDto> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |state, h| {
            let r = state.engine.merge(&h.session, &opts.to_model())?;
            let _ = status_dto(state, h);
            Ok(MergeResultDto::from_model(&r))
        })
    })
    .await
}

// ---------- Git 身份（FR-4.2 / FR-10.3） ----------

#[tauri::command]
pub async fn get_effective_identity(app: AppHandle) -> CmdResult<Option<IdentityDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let guard = state.session.lock().unwrap();
        let repo = guard.as_ref().map(|h| h.path.clone());
        // 1) 仓库设置覆盖
        if let Some(repo) = &repo {
            let cfg = state.config.lock().unwrap();
            if let Some(e) = cfg.recent_repos.iter().find(|e| &e.path == repo) {
                if let Some(id) = &e.identity {
                    if !id.name.trim().is_empty() && !id.email.trim().is_empty() {
                        return Ok(Some(IdentityDto {
                            name: id.name.clone(),
                            email: id.email.clone(),
                        }));
                    }
                }
            }
        }
        // 2) git 配置（仓库级 → 全局）
        if let Some(h) = guard.as_ref() {
            if let Ok(Some(id)) = state.engine.identity(&h.session) {
                return Ok(Some(IdentityDto::from(&id)));
            }
        }
        // 3) 全局
        if let Ok(Some(id)) = state.engine.global_identity() {
            return Ok(Some(IdentityDto::from(&id)));
        }
        Ok(None)
    })
    .await
}

#[tauri::command]
pub async fn get_global_identity(app: AppHandle) -> CmdResult<Option<IdentityDto>> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let id = state.engine.global_identity()?;
        Ok(id.as_ref().map(IdentityDto::from))
    })
    .await
}

#[tauri::command]
pub async fn set_identity(app: AppHandle, scope: String, identity: IdentityReq) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let id = Identity {
            name: identity.name,
            email: identity.email,
        };
        if scope == "global" {
            state.engine.set_global_identity(&id)?;
            Ok(())
        } else {
            with_session(&state, |_state, h| {
                state
                    .engine
                    .set_identity(&h.session, IdentityScope::Repo, &id)?;
                Ok(())
            })
        }
    })
    .await
}

#[tauri::command]
pub async fn clear_repo_identity(app: AppHandle) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        with_session(&state, |_state, h| {
            state.engine.clear_repo_identity(&h.session)?;
            Ok(())
        })
    })
    .await
}

// ---------- 外部工具 / 诊断（FR-9.1 / FR-10.5 / FR-10.7） ----------

#[tauri::command]
pub async fn open_in_explorer(path: String) -> CmdResult<()> {
    run_blocking(move || {
        platform::reveal_in_explorer(&to_path(&path))
            .map_err(|e| CommandError::msg("unknown", e.to_string()))
    })
    .await
}

/// 打开配置目录（工具菜单）：config.json 所在目录。
#[tauri::command]
pub async fn open_config_dir() -> CmdResult<()> {
    run_blocking(move || {
        let Some(path) = higit_core::config::config_path() else {
            return Err(CommandError::msg("unknown", "config path unavailable"));
        };
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
        platform::reveal_in_explorer(&dir).map_err(|e| CommandError::msg("unknown", e.to_string()))
    })
    .await
}

/// 用系统默认浏览器打开 URL（仅允许 http/https 且不含空白字符，防注入）。
#[tauri::command]
pub async fn open_url(url: String) -> CmdResult<()> {
    run_blocking(move || {
        let ok = (url.starts_with("http://") || url.starts_with("https://"))
            && !url.chars().any(|c| c.is_whitespace() || c.is_control());
        if !ok {
            return Err(CommandError::msg("unknown", "invalid url"));
        }
        platform::open_url(&url).map_err(|e| CommandError::msg("unknown", e.to_string()))
    })
    .await
}

#[tauri::command]
pub async fn open_in_terminal(app: AppHandle) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let dir = state
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|h| h.path.clone());
        match dir {
            Some(d) => platform::open_in_terminal(&d)
                .map_err(|e| CommandError::msg("unknown", e.to_string())),
            None => Err(no_repo()),
        }
    })
    .await
}

#[tauri::command]
pub async fn edit_gitignore(app: AppHandle) -> CmdResult<()> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let dir = state
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|h| h.path.clone());
        match dir {
            Some(d) => {
                let file = d.join(".gitignore");
                if !file.exists() {
                    std::fs::write(&file, "")
                        .map_err(|e| CommandError::msg("unknown", e.to_string()))?;
                }
                platform::open_in_editor(&file)
                    .map_err(|e| CommandError::msg("unknown", e.to_string()))
            }
            None => Err(no_repo()),
        }
    })
    .await
}

#[tauri::command]
pub async fn export_diagnostics(app: AppHandle) -> CmdResult<String> {
    run_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let state = app.state::<AppState>();
        let version = app.package_info().version.to_string();
        let text = {
            let cfg = state.config.lock().unwrap();
            format!(
                "HiGit diagnostics\n=================\nVersion: {version}\nOS: {} {}\nGit backend: {}\nLanguage: {}\nTheme: {}\nAuto refresh: {}\nShow ignored: {}\nProxy configured: {}\nRepositories ({}):\n{}\nConfig: {}\n",
                std::env::consts::OS,
                std::env::consts::ARCH,
                cfg.git_backend,
                if cfg.language.is_empty() { "(system)" } else { &cfg.language },
                cfg.theme,
                cfg.auto_refresh,
                cfg.show_ignored,
                cfg.proxy_enabled && !cfg.proxy.trim().is_empty(),
                cfg.recent_repos.len(),
                cfg.recent_repos
                    .iter()
                    .map(|e| format!("  - {} ({})", e.display_name(), e.path.display()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                higit_core::config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            )
        };

        let file = app
            .dialog()
            .file()
            .add_filter("Text", &["txt"])
            .set_file_name("HiGit-diagnostics.txt")
            .blocking_save_file();
        let Some(file) = file else { return Ok(String::new()) };
        let path: PathBuf = file.as_path().map(|p| p.to_path_buf()).unwrap_or_default();
        std::fs::write(&path, text).map_err(|e| CommandError::msg("unknown", e.to_string()))?;
        Ok(abs_string(&path))
    })
    .await
}

#[cfg(test)]
mod net_note_tests {
    use super::{auth_note, egress_note, socks_proxy_hint, stage_note};
    use higit_git::engine::Credentials;

    #[test]
    fn egress_https_direct_when_proxy_off() {
        let zh = egress_note("https://github.com/hicode0101/HiGit.git", None, true);
        assert_eq!(zh, "出网方式: 直连");
        let en = egress_note("https://github.com/hicode0101/HiGit.git", None, false);
        assert_eq!(en, "Network: direct connection");
    }

    #[test]
    fn egress_https_via_proxy_when_enabled() {
        let zh = egress_note(
            "https://github.com/x/y.git",
            Some("socks5://127.0.0.1:1080"),
            true,
        );
        assert!(zh.contains("走代理"), "zh: {zh}");
        assert!(zh.contains("socks5://127.0.0.1:1080"), "zh: {zh}");
        let en = egress_note(
            "https://github.com/x/y.git",
            Some("http://127.0.0.1:7891"),
            false,
        );
        assert!(en.contains("proxy http://127.0.0.1:7891"), "en: {en}");
    }

    #[test]
    fn egress_ssh_is_direct() {
        let zh = egress_note(
            "git@github.com:hicode0101/test1.git",
            Some("http://127.0.0.1:7891"),
            true,
        );
        assert!(zh.contains("直连"), "SSH 代理设置不生效，应为直连: {zh}");
        let en = egress_note("ssh://git@github.com/x/y.git", None, false);
        assert!(en.contains("direct"), "en: {en}");
    }

    #[test]
    fn egress_local_path_has_no_network() {
        let zh = egress_note("D:/Code/some-repo", None, true);
        assert!(zh.contains("本地路径"), "zh: {zh}");
        let en = egress_note("file:///D:/Code/some-repo", None, false);
        assert!(en.contains("local path"), "en: {en}");
    }

    #[test]
    fn auth_note_matches_credential_chain() {
        let ssh = Credentials::SshKey {
            username: "git".to_string(),
            key_path: "D:/keys/id_ed25519".to_string(),
            passphrase: String::new(),
            from_store: false,
        };
        assert!(auth_note(Some(&ssh), true).contains("SSH 密钥 D:/keys/id_ed25519"));
        let agent = Credentials::SshKey {
            username: "git".to_string(),
            key_path: String::new(),
            passphrase: String::new(),
            from_store: false,
        };
        assert!(auth_note(Some(&agent), true).contains("ssh-agent"));
        assert!(auth_note(Some(&agent), false).contains("ssh-agent"));
        let http = Credentials::UserPass {
            username: "hicode0101".to_string(),
            password: "secret".to_string(),
        };
        let line = auth_note(Some(&http), true);
        assert!(
            line.contains("HTTPS") && line.contains("hicode0101"),
            "{line}"
        );
        // 日志不得包含密码
        assert!(!line.contains("secret"));
        assert!(auth_note(None, true).contains("默认"));
        assert!(auth_note(None, false).contains("default"));
    }

    #[test]
    fn stage_note_maps_known_stages() {
        assert_eq!(stage_note("Receiving objects", true), "正在接收对象…");
        assert_eq!(stage_note("Resolving deltas", false), "Resolving deltas…");
        assert_eq!(stage_note("Packing objects", true), "正在打包对象…");
        assert_eq!(stage_note("Pushing objects", true), "正在推送对象…");
        assert_eq!(stage_note("Other stage", true), "Other stage");
    }

    #[test]
    fn socks_proxy_hint_only_for_socks_urls() {
        assert!(socks_proxy_hint(Some("socks5://127.0.0.1:10808"), true).is_some());
        assert!(socks_proxy_hint(Some("SOCKS5://127.0.0.1:10808"), true).is_some());
        assert!(socks_proxy_hint(Some("socks5h://x:1"), false).is_some());
        assert!(socks_proxy_hint(Some("http://127.0.0.1:7891"), true).is_none());
        assert!(socks_proxy_hint(None, true).is_none());
        let zh = socks_proxy_hint(Some("socks5://127.0.0.1:10808"), true).unwrap();
        assert!(zh.contains("HTTP"), "{zh}");
    }
}

#[cfg(test)]
mod row_status_kind_tests {
    use std::collections::{HashMap, HashSet};

    use super::row_status_kind;
    use higit_git::model::FileStatusKind;

    fn status_map(pairs: &[(&str, FileStatusKind)]) -> HashMap<String, FileStatusKind> {
        pairs.iter().map(|(p, k)| (p.to_string(), *k)).collect()
    }

    #[test]
    fn file_inside_folded_untracked_dir_is_untracked() {
        // status 对整目录未跟踪只报一条 "gfe-analysis"，内部文件必须继承"新增"
        let map = status_map(&[("gfe-analysis", FileStatusKind::Untracked)]);
        let dir_changed = HashSet::new();
        assert_eq!(
            row_status_kind(&map, &dir_changed, "gfe-analysis/file-management.js", false),
            FileStatusKind::Untracked
        );
        // 深层同理
        assert_eq!(
            row_status_kind(&map, &dir_changed, "gfe-analysis/a/b/c.js", false),
            FileStatusKind::Untracked
        );
    }

    #[test]
    fn subdir_inside_folded_untracked_dir_is_untracked() {
        let map = status_map(&[("gfe-analysis", FileStatusKind::Untracked)]);
        let dir_changed = HashSet::new();
        assert_eq!(
            row_status_kind(&map, &dir_changed, "gfe-analysis/sub", true),
            FileStatusKind::Untracked
        );
    }

    #[test]
    fn file_inside_folded_ignored_dir_is_ignored() {
        let map = status_map(&[("vendor", FileStatusKind::Ignored)]);
        let dir_changed = HashSet::new();
        assert_eq!(
            row_status_kind(&map, &dir_changed, "vendor/lib.js", false),
            FileStatusKind::Ignored
        );
    }

    #[test]
    fn direct_hit_and_dir_aggregation_unchanged() {
        let map = status_map(&[
            ("src/main.rs", FileStatusKind::Modified),
            ("new.txt", FileStatusKind::Untracked),
        ]);
        let mut dir_changed = HashSet::new();
        dir_changed.insert("src".to_string());
        // 命中自身条目
        assert_eq!(
            row_status_kind(&map, &dir_changed, "src/main.rs", false),
            FileStatusKind::Modified
        );
        // 目录聚合：后代有变更 → 已修改
        assert_eq!(
            row_status_kind(&map, &dir_changed, "src", true),
            FileStatusKind::Modified
        );
        // 无任何关联 → 未修改
        assert_eq!(
            row_status_kind(&map, &dir_changed, "other.txt", false),
            FileStatusKind::Clean
        );
        // 自身未跟踪
        assert_eq!(
            row_status_kind(&map, &dir_changed, "new.txt", false),
            FileStatusKind::Untracked
        );
    }

    #[test]
    fn non_inheritable_ancestor_does_not_leak() {
        // 子模块等目录条目为 Modified：内部文件不继承，保持未修改
        let map = status_map(&[("sub", FileStatusKind::Modified)]);
        let dir_changed = HashSet::new();
        assert_eq!(
            row_status_kind(&map, &dir_changed, "sub/x.txt", false),
            FileStatusKind::Clean
        );
    }

    #[test]
    fn ignored_passes_through_for_display() {
        // show_ignored=true 时忽略条目应显示"忽略"而不是"未修改"
        let map = status_map(&[("build.log", FileStatusKind::Ignored)]);
        let dir_changed = HashSet::new();
        assert_eq!(
            row_status_kind(&map, &dir_changed, "build.log", false),
            FileStatusKind::Ignored
        );
    }
}
