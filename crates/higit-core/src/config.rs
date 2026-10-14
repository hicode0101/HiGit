//! 应用配置（docs/technical-design.md §4.5）。
//! 存放于系统标准配置目录：Windows %APPDATA%\HiGit\config.json，
//! macOS ~/Library/Application Support/HiGit/，Linux ~/.config/HiGit/。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 本仓库 Git 身份（仓库设置对话框“Git 身份”页）：提交时的作者参数。
/// 不落 git 配置之外的敏感信息，纯文本 name / email。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIdentity {
    pub name: String,
    pub email: String,
}

/// 单仓库认证配置（仓库设置对话框）：只保存非敏感信息，
/// 密码 / Token / 密钥口令等机密仍走系统凭证管理器（FR-5.5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoAuth {
    /// "http" | "ssh"
    pub kind: String,
    /// 用户名（SSH 通常为 "git"；空 = 按 URL 推断）
    #[serde(default)]
    pub username: String,
    /// SSH 私钥路径（空 = ssh-agent / 默认密钥）
    #[serde(default)]
    pub key_path: String,
}

/// 最近仓库条目（FR-1.7）。别名 / 远端 URL / 认证 / Git 身份统一在此保存，
/// 提交 / 推送 / 拉取所需参数以此处（仓库设置）为主（见 state.rs `repo_sync_params`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// 唯一标识（uuid v4，随机生成）：跨同名 / 同别名稳定定位单个仓库条目；
    /// 旧版配置无此字段，加载 / 插入时自动补齐
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// 用户自定义显示别名（P1）；空 = 使用目录名
    #[serde(default)]
    pub alias: Option<String>,
    /// 本仓库认证覆盖（仓库设置对话框）；None = 未单独配置
    #[serde(default)]
    pub auth: Option<RepoAuth>,
    /// 推送 / 拉取的目标远端 URL（仓库设置对话框）；空 = 跟随仓库 git 配置
    #[serde(default)]
    pub url: String,
    /// 本仓库提交身份（仓库设置对话框）；None = 跟随 git 配置（仓库级 → 全局）
    #[serde(default)]
    pub identity: Option<RepoIdentity>,
}

/// 生成仓库唯一 ID（uuid v4，32 位十六进制无连字符）。
pub fn gen_repo_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl RepoEntry {
    /// 列表展示名：用户别名优先，其次目录名。
    pub fn display_name(&self) -> &str {
        self.alias
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// UI 语言："en" | "zh-CN"；空串表示跟随系统
    pub language: String,
    /// 主题："light" | "dark" | "system"
    pub theme: String,
    /// 仓库列表（按添加顺序稳定排列，上限 20，点击切换不改变顺序）
    pub recent_repos: Vec<RepoEntry>,
    /// 上次打开的仓库路径（FR-1.7 重启恢复）；空 = 无
    pub last_repo: String,
    /// 文件变更自动刷新（FR-2.5）
    pub auto_refresh: bool,
    /// 显示被忽略文件（FR-2.6）
    pub show_ignored: bool,
    /// Git 后端选择（预留，见 technical-design §4.1）
    pub git_backend: String,
    /// 网络代理 URL（FR-10.4）：空 = 直连；如 socks5://127.0.0.1:10808
    pub proxy: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            language: String::new(),
            theme: "system".to_string(),
            recent_repos: Vec::new(),
            last_repo: String::new(),
            auto_refresh: true,
            show_ignored: false,
            git_backend: "git2".to_string(),
            proxy: String::new(),
        }
    }
}

impl AppConfig {
    /// 确保仓库在列表中：已存在则保持原位（仅刷新名称），新仓库追加到末尾。
    pub fn upsert_recent_repo(&mut self, mut entry: RepoEntry) {
        if entry.id.is_empty() {
            entry.id = gen_repo_id();
        }
        match self.recent_repos.iter_mut().find(|e| e.path == entry.path) {
            Some(existing) => existing.name = entry.name,
            None => {
                self.recent_repos.push(entry);
                if self.recent_repos.len() > 20 {
                    self.recent_repos.remove(0);
                }
            }
        }
    }
}

/// 默认配置文件路径；无法定位系统配置目录时为 None。
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("HiGit").join("config.json"))
}

/// 从磁盘加载；文件不存在返回 Ok(None)，格式损坏返回错误（上层可提示并回退默认值）。
pub fn load_from(path: &Path) -> io::Result<Option<AppConfig>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut cfg: AppConfig = serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            ensure_repo_ids(&mut cfg);
            Ok(Some(cfg))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 旧版配置的仓库条目没有 id（或手工编辑出现重复）：加载期补齐 / 去重，随下次保存持久化。
fn ensure_repo_ids(cfg: &mut AppConfig) {
    let mut seen = std::collections::HashSet::new();
    for e in &mut cfg.recent_repos {
        if e.id.is_empty() || !seen.insert(e.id.clone()) {
            e.id = gen_repo_id();
            seen.insert(e.id.clone());
        }
    }
}

pub fn save_to(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_recent_repos_upsert() {
        let dir = std::env::temp_dir().join(format!("higit-cfg-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);

        let mut cfg = AppConfig {
            language: "zh-CN".into(),
            theme: "dark".into(),
            ..Default::default()
        };
        cfg.upsert_recent_repo(RepoEntry {
            id: String::new(),
            name: "A".into(),
            path: "/tmp/a".into(),
            alias: None,
            auth: None,
            url: String::new(),
            identity: None,
        });
        cfg.upsert_recent_repo(RepoEntry {
            id: String::new(),
            name: "B".into(),
            path: "/tmp/b".into(),
            alias: None,
            auth: None,
            url: String::new(),
            identity: None,
        });
        cfg.upsert_recent_repo(RepoEntry {
            id: String::new(),
            name: "A2".into(),
            path: "/tmp/a".into(),
            alias: None,
            auth: None,
            url: String::new(),
            identity: None,
        });
        cfg.last_repo = "/tmp/b".into();

        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded, cfg);
        // 顺序稳定：重复打开不改变位置，仅刷新名称
        assert_eq!(loaded.recent_repos.len(), 2);
        assert_eq!(loaded.recent_repos[0].path, PathBuf::from("/tmp/a"));
        assert_eq!(loaded.recent_repos[0].name, "A2");
        assert_eq!(loaded.recent_repos[1].path, PathBuf::from("/tmp/b"));
        assert_eq!(loaded.last_repo, "/tmp/b");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_last_repo_field_falls_back_to_default() {
        // 旧版配置无 last_repo 字段：容器 serde(default) 应以空串补齐
        let legacy = r#"{"language":"","theme":"system","recent_repos":[],"auto_refresh":true,"show_ignored":false,"git_backend":"git2","proxy":""}"#;
        let cfg: AppConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(cfg.last_repo, "");
    }

    #[test]
    fn missing_file_returns_none() {
        let path = std::env::temp_dir().join("higit-cfg-missing-does-not-exist.json");
        let _ = std::fs::remove_file(&path);
        assert!(load_from(&path).unwrap().is_none());
    }

    #[test]
    fn legacy_entries_get_unique_ids_on_load() {
        // 旧版配置条目无 id（或重复）：加载期补齐且互不相同
        let dir = std::env::temp_dir().join(format!("higit-cfg-id-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut cfg = AppConfig::default();
        cfg.recent_repos.push(RepoEntry {
            id: String::new(),
            name: "a".into(),
            path: "/tmp/a".into(),
            alias: None,
            auth: None,
            url: String::new(),
            identity: None,
        });
        cfg.recent_repos.push(RepoEntry {
            id: String::new(),
            name: "b".into(),
            path: "/tmp/b".into(),
            alias: None,
            auth: None,
            url: String::new(),
            identity: None,
        });
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert!(loaded.recent_repos.iter().all(|e| !e.id.is_empty()));
        assert_ne!(loaded.recent_repos[0].id, loaded.recent_repos[1].id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_auth_and_alias_roundtrip() {
        let dir = std::env::temp_dir().join(format!("higit-cfg-auth-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);

        let mut cfg = AppConfig::default();
        cfg.upsert_recent_repo(RepoEntry {
            id: String::new(),
            name: "test1".into(),
            path: "/tmp/test1".into(),
            alias: Some("My Test".into()),
            auth: Some(RepoAuth {
                kind: "ssh".into(),
                username: "git".into(),
                key_path: "D:\\keys\\id_ed25519".into(),
            }),
            url: "git@github.com:hicode0101/test1.git".into(),
            identity: Some(RepoIdentity {
                name: "dev".into(),
                email: "dev@example.com".into(),
            }),
        });
        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded, cfg);
        // 别名优先展示；未设别名回退目录名
        assert_eq!(loaded.recent_repos[0].display_name(), "My Test");
        let auth = loaded.recent_repos[0].auth.as_ref().unwrap();
        assert_eq!(auth.kind, "ssh");
        assert_eq!(auth.key_path, "D:\\keys\\id_ed25519");
        // 远端 URL 与提交身份同属仓库设置参数，一并持久化
        assert_eq!(
            loaded.recent_repos[0].url,
            "git@github.com:hicode0101/test1.git"
        );
        let id = loaded.recent_repos[0].identity.as_ref().unwrap();
        assert_eq!(id.name, "dev");
        assert_eq!(id.email, "dev@example.com");

        // 旧版配置无 alias / auth / url / identity 字段：serde(default) 补齐
        let legacy = r#"{"name":"old","path":"/tmp/old"}"#;
        let entry: RepoEntry = serde_json::from_str(legacy).unwrap();
        assert_eq!(entry.alias, None);
        assert_eq!(entry.auth, None);
        assert_eq!(entry.url, "");
        assert_eq!(entry.identity, None);
        assert_eq!(entry.display_name(), "old");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
