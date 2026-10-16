//! 前后端传输模型（camelCase JSON）；由引擎层纯 Rust 模型转换而来。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use higit_git::model::{
    BranchInfo, CommitDetail, CommitEntry, CommitFile, CommitFileKind, DiffLine, DiffLineOrigin,
    FileStatusKind, Identity, MergeOutcome, RepoStatus, TextDiff,
};

pub fn kind_str(k: FileStatusKind) -> String {
    match k {
        FileStatusKind::Clean => "clean".into(),
        FileStatusKind::Modified => "modified".into(),
        FileStatusKind::Staged => "staged".into(),
        FileStatusKind::Untracked => "untracked".into(),
        FileStatusKind::Deleted => "deleted".into(),
        FileStatusKind::Renamed => "renamed".into(),
        FileStatusKind::Conflicted => "conflicted".into(),
        FileStatusKind::Ignored => "ignored".into(),
    }
}

/// 路径 → 相对路径字符串（统一正斜杠，git 惯例，展示/回传一致）
pub fn rel_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 绝对路径 → 平台原生分隔符字符串（Windows 反斜杠，macOS/Linux 正斜杠）
pub fn abs_string(p: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        p.to_string_lossy().replace('/', "\\")
    }
    #[cfg(not(target_os = "windows"))]
    {
        p.to_string_lossy().replace('\\', "/")
    }
}

/// 还原前端回传的路径字符串为平台 PathBuf（分隔符按平台规范化）
pub fn to_path(s: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(s.replace('/', "\\"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from(s)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    /// 相对仓库根的路径（正斜杠）
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadDto {
    pub short_id: String,
    pub summary: String,
    pub time_unix: i64,
    pub committer_email: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub upstream: bool,
    pub head: Option<HeadDto>,
    pub entries: Vec<EntryDto>,
}

impl StatusDto {
    pub fn from_model(s: &RepoStatus) -> Self {
        Self {
            branch: s.branch.clone(),
            ahead: s.ahead,
            behind: s.behind,
            upstream: s.upstream,
            head: s.head.as_ref().map(|h| HeadDto {
                short_id: h.short_id.clone(),
                summary: h.summary.clone(),
                time_unix: h.time_unix,
                committer_email: h.committer_email.clone(),
            }),
            entries: s
                .entries
                .iter()
                .map(|e| EntryDto {
                    path: rel_string(&e.path),
                    kind: kind_str(e.kind),
                })
                .collect(),
        }
    }
}

/// 文件列表行（目录浏览 + Git 状态聚合），由后端合并文件系统与状态缓存。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRowDto {
    pub name: String,
    /// 相对仓库根的路径（正斜杠；目录不带尾斜杠）
    pub rel_path: String,
    /// 绝对路径（前端双击进目录 / 打开差异用）
    pub abs_path: String,
    pub is_dir: bool,
    pub kind: String,
    /// 文件大小（字节；目录 / 已删除的文件为 null）
    pub size: Option<u64>,
    pub mtime_unix: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeRowDto {
    /// 绝对路径（展开状态以它为键）
    pub abs_path: String,
    pub name: String,
    pub level: u32,
    /// 根级 .git 元目录（特殊图标）
    pub git_meta: bool,
    /// 目录下是否还有子目录（图标区分可展开 / 叶子目录）
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummaryDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub alias: Option<String>,
    pub branch: Option<String>,
    /// 未提交变更数（忽略项不计）
    pub dirty: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDto {
    pub name: String,
    pub email: String,
}

impl From<&Identity> for IdentityDto {
    fn from(i: &Identity) -> Self {
        Self {
            name: i.name.clone(),
            email: i.email.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityReq {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthCfgDto {
    /// "http" | "ssh"
    pub kind: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub key_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoEntryDto {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthCfgDto>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub identity: Option<IdentityDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDto {
    pub language: String,
    pub theme: String,
    pub recent_repos: Vec<RepoEntryDto>,
    pub last_repo: String,
    pub auto_refresh: bool,
    pub show_ignored: bool,
    pub git_backend: String,
    #[serde(default)]
    pub proxy_enabled: bool,
    pub proxy: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLineDto {
    /// "context" | "add" | "del" | "hunk"
    pub origin: String,
    pub text: String,
}

fn origin_str(o: DiffLineOrigin) -> String {
    match o {
        DiffLineOrigin::Context => "context".into(),
        DiffLineOrigin::Addition => "add".into(),
        DiffLineOrigin::Deletion => "del".into(),
        DiffLineOrigin::HunkHeader => "hunk".into(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffDto {
    pub lines: Vec<DiffLineDto>,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub truncated: bool,
    pub too_large: bool,
}

impl DiffDto {
    pub fn from_model(d: &TextDiff) -> Self {
        Self {
            lines: d
                .lines
                .iter()
                .map(|l: &DiffLine| DiffLineDto {
                    origin: origin_str(l.origin),
                    text: l.text.clone(),
                })
                .collect(),
            additions: d.additions,
            deletions: d.deletions,
            is_binary: d.is_binary,
            truncated: d.truncated,
            too_large: d.too_large,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitEntryDto {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub time_unix: i64,
    pub refs: Vec<String>,
    pub parents: usize,
}

impl CommitEntryDto {
    pub fn from_model(c: &CommitEntry) -> Self {
        Self {
            id: c.id.clone(),
            short_id: c.short_id.clone(),
            summary: c.summary.clone(),
            author_name: c.author_name.clone(),
            author_email: c.author_email.clone(),
            time_unix: c.time_unix,
            refs: c.refs.clone(),
            parents: c.parents,
        }
    }
}

fn file_kind_str(k: CommitFileKind) -> String {
    match k {
        CommitFileKind::Added => "added".into(),
        CommitFileKind::Modified => "modified".into(),
        CommitFileKind::Deleted => "deleted".into(),
        CommitFileKind::Renamed => "renamed".into(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileDto {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetailDto {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
    pub time_unix: i64,
    pub parents: Vec<String>,
    pub files: Vec<CommitFileDto>,
}

impl CommitDetailDto {
    pub fn from_model(d: &CommitDetail) -> Self {
        Self {
            id: d.id.clone(),
            short_id: d.short_id.clone(),
            summary: d.summary.clone(),
            message: d.message.clone(),
            author_name: d.author_name.clone(),
            author_email: d.author_email.clone(),
            committer_name: d.committer_name.clone(),
            committer_email: d.committer_email.clone(),
            time_unix: d.time_unix,
            parents: d.parents.clone(),
            files: d
                .files
                .iter()
                .map(|f: &CommitFile| CommitFileDto {
                    path: rel_string(&f.path),
                    old_path: f.old_path.as_ref().map(|p| rel_string(p)),
                    kind: file_kind_str(f.kind),
                    additions: f.additions,
                    deletions: f.deletions,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDto {
    pub name: String,
    pub is_remote: bool,
    pub is_current: bool,
    pub short_id: String,
    pub last_commit_summary: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

impl BranchDto {
    pub fn from_model(b: &BranchInfo) -> Self {
        Self {
            name: b.name.clone(),
            is_remote: b.is_remote,
            is_current: b.is_current,
            short_id: b.short_id.clone(),
            last_commit_summary: b.last_commit_summary.clone(),
            upstream: b.upstream.clone(),
            ahead: b.ahead,
            behind: b.behind,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutcomeDto {
    pub fast_forward: bool,
    pub updated: bool,
    pub conflicts: Vec<String>,
}

impl MergeOutcomeDto {
    pub fn from_model(m: &MergeOutcome) -> Self {
        Self {
            fast_forward: m.fast_forward,
            updated: m.updated,
            conflicts: m.conflicts.iter().map(|p| rel_string(p)).collect(),
        }
    }
}

/// 网络凭证（前端凭证对话框 / 克隆对话框提交）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredsDto {
    /// "http" | "ssh"
    pub kind: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub key_path: String,
    #[serde(default)]
    pub passphrase: String,
}

impl CredsDto {
    pub fn into_engine(self) -> higit_git::engine::Credentials {
        if self.kind == "ssh" {
            higit_git::engine::Credentials::SshKey {
                username: self.username,
                key_path: self.key_path,
                passphrase: self.passphrase,
                from_store: false,
            }
        } else {
            higit_git::engine::Credentials::UserPass {
                username: self.username,
                password: self.password,
            }
        }
    }
}

/// 打开的仓库视图（open_repo / new_repo 的返回）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoViewDto {
    pub path: String,
    pub name: String,
    pub status: StatusDto,
}
