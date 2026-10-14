//! 命令层错误：把 [`higit_git::error::HigitError`] 序列化为结构化 JSON，
//! 前端按 `code + params` 做 i18n 渲染，`raw` 进"详情"可复制（FR-12.4）。

use serde::Serialize;

use higit_git::error::{AuthKind, HigitError};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthDetailDto {
    /// "http" | "ssh"
    pub kind: String,
    pub host: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// 结构化错误码（"auth-failed" / "merge-conflict" …），前端映射本地化文案
    pub code: String,
    /// 插值参数（路径、分支名等）
    pub params: Vec<String>,
    /// 认证上下文（仅 AuthFailed 携带，驱动凭证对话框）
    pub auth: Option<AuthDetailDto>,
    /// 原始错误消息（进详情，可复制）
    pub raw: Option<String>,
}

impl From<HigitError> for CommandError {
    fn from(e: HigitError) -> Self {
        Self {
            code: e.code.to_string(),
            params: e.params,
            auth: e.auth.map(|a| AuthDetailDto {
                kind: match a.kind {
                    AuthKind::Http => "http".to_string(),
                    AuthKind::Ssh => "ssh".to_string(),
                },
                host: a.host,
                username: a.username,
            }),
            raw: e.raw,
        }
    }
}

impl CommandError {
    pub fn msg(code: &str, raw: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            params: Vec::new(),
            auth: None,
            raw: Some(raw.into()),
        }
    }
}

pub type CmdResult<T> = Result<T, CommandError>;
