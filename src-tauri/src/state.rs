//! 应用全局状态：引擎、配置、当前会话（含文件监听）、长任务注册表。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use higit_core::config::AppConfig;
use higit_git::engine::{CancelToken, GitEngine, RepoSession};

/// 打开的仓库会话 + 文件监听句柄。
pub struct SessionHandle {
    pub session: RepoSession,
    /// 仓库根（绝对路径）
    pub path: PathBuf,
    /// 最近一次 status 结果（文件列表聚合 / 暂存展开用）
    pub last_status: Option<higit_git::model::RepoStatus>,
    /// watcher 保活 + 停止标记；drop 时由 close_repo 显式置位
    pub watch: Option<WatchHandle>,
}

pub struct WatchHandle {
    pub stop: Arc<AtomicBool>,
    pub(crate) _watcher: notify::RecommendedWatcher,
}

impl WatchHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl SessionHandle {
    /// 关闭监听（切换 / 关闭仓库时调用）
    pub fn stop_watch(&self) {
        if let Some(w) = &self.watch {
            w.stop();
        }
    }
}

pub struct JobHandle {
    pub cancel: CancelToken,
    /// "clone" | "push" | "pull" | "fetch"（任务种类标记）
    #[allow(dead_code)]
    pub kind: &'static str,
}

pub struct AppState {
    pub engine: Arc<dyn GitEngine>,
    pub config: Mutex<AppConfig>,
    pub session: Mutex<Option<SessionHandle>>,
    pub jobs: Mutex<HashMap<u32, JobHandle>>,
    next_job: AtomicU32,
    /// Git 写操作 / 长任务进行中：暂停自动刷新
    pub write_busy: AtomicBool,
}

impl AppState {
    pub fn new(engine: Arc<dyn GitEngine>, config: AppConfig) -> Self {
        Self {
            engine,
            config: Mutex::new(config),
            session: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            next_job: AtomicU32::new(1),
            write_busy: AtomicBool::new(false),
        }
    }

    pub fn save_config(&self) {
        let cfg = self.config.lock().unwrap();
        if let Some(path) = higit_core::config::config_path() {
            let _ = higit_core::config::save_to(&path, &cfg);
        }
    }

    pub fn register_job(&self, kind: &'static str) -> (u32, CancelToken) {
        let id = self.next_job.fetch_add(1, Ordering::SeqCst);
        let cancel = CancelToken::default();
        self.jobs.lock().unwrap().insert(
            id,
            JobHandle {
                cancel: cancel.clone(),
                kind,
            },
        );
        (id, cancel)
    }

    pub fn finish_job(&self, id: u32) {
        self.jobs.lock().unwrap().remove(&id);
    }
}

/// 加载磁盘配置；文件缺失返回默认，损坏时保留默认（前端会提示）。
pub fn load_config() -> (AppConfig, bool) {
    let Some(path) = higit_core::config::config_path() else {
        return (AppConfig::default(), false);
    };
    match higit_core::config::load_from(&path) {
        Ok(Some(cfg)) => (cfg, false),
        Ok(None) => (AppConfig::default(), false),
        Err(_) => (AppConfig::default(), true),
    }
}
