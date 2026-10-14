//! 结构化错误：核心层只回传错误码 + 插值参数 + 原始消息，
//! 用户可见文案由 UI 层按 `code` 做 i18n 渲染（docs/technical-design.md §4.2）。

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// 功能尚未实现（后端缺口或里程碑未到）
    NotImplemented,
    /// 无法打开仓库 / 不是有效的 Git 仓库
    InvalidRepository,
    /// 仓库存在但读取状态失败
    StatusFailed,
    /// 提交者身份未配置（user.name / user.email 缺失）
    IdentityMissing,
    /// 认证失败
    AuthFailed,
    /// 网络 / 远端操作失败
    NetworkFailed,
    /// 合并冲突
    MergeConflict,
    /// 当前分支未设置上游（FR-5.1/5.2）
    NoUpstream,
    /// 本地与远端分叉，无法快进合并（FR-5.1）
    Diverged,
    /// 推送被拒：远端有本地没有的提交（FR-5.7，不提供强推）
    PushRejected,
    /// 工作区有未提交修改，与本操作冲突（切换 / 合并前拒绝执行）
    DirtyWorktree,
    /// 分支包含未合并提交，删除需强确认（FR-7.3）
    BranchNotMerged,
    /// 用户取消长任务
    Cancelled,
    /// 其他未分类错误
    Unknown,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            ErrorCode::NotImplemented => "not-implemented",
            ErrorCode::InvalidRepository => "invalid-repository",
            ErrorCode::StatusFailed => "status-failed",
            ErrorCode::IdentityMissing => "identity-missing",
            ErrorCode::AuthFailed => "auth-failed",
            ErrorCode::NetworkFailed => "network-failed",
            ErrorCode::MergeConflict => "merge-conflict",
            ErrorCode::NoUpstream => "no-upstream",
            ErrorCode::Diverged => "diverged",
            ErrorCode::PushRejected => "push-rejected",
            ErrorCode::DirtyWorktree => "dirty-worktree",
            ErrorCode::BranchNotMerged => "branch-not-merged",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// 认证方式（FR-5.5 HTTPS / FR-5.6 SSH）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Http,
    Ssh,
}

/// 认证上下文：凭证对话框按它填充字段（主机 / 用户名提示 / 认证方式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDetail {
    pub kind: AuthKind,
    pub host: String,
    /// URL 或服务端给出的用户名提示，可能为空
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct HigitError {
    pub code: ErrorCode,
    /// 插值参数（如仓库路径、分支名），供 UI 层填充本地化文案
    pub params: Vec<String>,
    /// 认证上下文（仅 AuthFailed 携带，驱动凭证对话框）
    pub auth: Option<AuthDetail>,
    /// 后端原始错误消息，UI 层放入“详情”供复制
    pub raw: Option<String>,
}

impl HigitError {
    pub fn new(code: ErrorCode, raw: impl Into<String>) -> Self {
        Self {
            code,
            params: Vec::new(),
            auth: None,
            raw: Some(raw.into()),
        }
    }

    pub fn not_implemented(what: &str) -> Self {
        Self {
            code: ErrorCode::NotImplemented,
            params: vec![what.to_string()],
            auth: None,
            raw: None,
        }
    }

    pub fn invalid_repository(path: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidRepository,
            params: vec![path.into()],
            auth: None,
            raw: Some(raw.into()),
        }
    }

    pub fn identity_missing() -> Self {
        Self {
            code: ErrorCode::IdentityMissing,
            params: Vec::new(),
            auth: None,
            raw: None,
        }
    }

    pub fn auth_failed(auth: AuthDetail, raw: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::AuthFailed,
            params: Vec::new(),
            auth: Some(auth),
            raw: Some(raw.into()),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            code: ErrorCode::Cancelled,
            params: Vec::new(),
            auth: None,
            raw: None,
        }
    }
}

impl fmt::Display for HigitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.code)?;
        if let Some(raw) = &self.raw {
            write!(f, " {raw}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HigitError {}

pub type Result<T> = std::result::Result<T, HigitError>;
