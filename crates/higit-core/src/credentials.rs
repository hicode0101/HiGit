//! 系统凭证管理器封装（FR-5.5 / FR-5.6）。
//!
//! 凭证写入系统安全存储：Windows 凭证管理器 / macOS 钥匙串 / Linux Secret Service，
//! 经 [`keyring`] crate 统一访问。存储键为 `HiGit` 服务 + `{username}@{host}`；
//! 值为 JSON 序列化的 [`StoredCredential`]。
//! 所有操作均为尽力而为：调用方对失败可静默降级（重新弹窗输入即可）。

use serde::{Deserialize, Serialize};

/// 服务名（凭证管理器中的条目前缀）。
const SERVICE: &str = "HiGit";

/// 一条已保存的凭证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    /// "http" | "ssh"
    pub kind: String,
    pub username: String,
    /// 密码 / Token（http）或密钥口令（ssh）
    pub secret: String,
    /// SSH 私钥路径（空 = ssh-agent / 默认密钥）
    #[serde(default)]
    pub key_path: String,
}

fn entry_key(host: &str, username: &str) -> String {
    let username = username.trim();
    if username.is_empty() {
        host.to_string()
    } else {
        format!("{username}@{host}")
    }
}

fn try_load(key: &str) -> Option<StoredCredential> {
    let entry = keyring::Entry::new(SERVICE, key).ok()?;
    entry
        .get_password()
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// 读取凭证：先按 `{hint}@{host}`，未命中再按 `{host}`（提示为空时只查后者）。
pub fn load(host: &str, username_hint: &str) -> Option<StoredCredential> {
    if let Some(c) = try_load(&entry_key(host, username_hint)) {
        return Some(c);
    }
    if !username_hint.trim().is_empty() {
        return try_load(&entry_key(host, ""));
    }
    None
}

/// 保存凭证（覆盖同键旧值）。
pub fn store(cred: &StoredCredential, host: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, &entry_key(host, &cred.username))
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string(cred).map_err(|e| e.to_string())?;
    entry.set_password(&json).map_err(|e| e.to_string())
}

/// 删除凭证（认证失败时清理过期记录用）。
pub fn forget(host: &str, username: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(SERVICE, &entry_key(host, username)).map_err(|e| e.to_string())?;
    entry.delete_credential().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_key_uses_username_hint() {
        assert_eq!(entry_key("github.com", ""), "github.com");
        assert_eq!(entry_key("github.com", " alice "), "alice@github.com");
    }
}
