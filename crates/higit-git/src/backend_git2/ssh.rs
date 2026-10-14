//! SSH 主机密钥 TOFU 校验（FR-1.2 SSH 克隆的前置条件）。
//!
//! libgit2 默认按 `~/.ssh/known_hosts` 严格校验：主机无记录时在认证之前就报
//! "invalid or unknown remote ssh hostkey"——纯 GUI 用户无法先手动 ssh-keyscan。
//! 此处接管校验，语义对齐 OpenSSH（条目为 (主机, 算法) 对）：
//! - `(host, 算法)` 已记录且密钥一致 → 放行；
//! - `(host, 算法)` 已记录但不一致 → 拒绝（密钥变更 / 疑似中间人，硬失败）；
//! - 未记录 → 接受并追加到 known_hosts（TOFU，等价 `ssh accept-new`），
//!   与 OpenSSH CLI 共享同一份文件。

use std::path::Path;

/// 主机密钥校验结论。
pub(crate) enum HostKeyVerdict {
    /// (host, 算法) 已记录且密钥一致
    Match,
    /// 未记录（host 全新，或 host 已知但该算法的密钥首次出现）
    Unknown,
    /// (host, 算法) 已记录但密钥不一致：疑似中间人，必须拒绝
    Mismatch,
}

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码（含 `=` 填充，与 OpenSSH known_hosts 一致）。
pub(crate) fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).map_or(0, |&v| v as u32);
        let b2 = chunk.get(2).map_or(0, |&v| v as u32);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// base64 解码：容忍缺失填充与换行空白；含非法字符时返回 None。
pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&c| !matches!(c, b'=' | b'\r' | b'\n' | b' '))
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// known_hosts 主机标记：22 端口用裸主机名，非 22 用 `[host]:port`（OpenSSH 约定）。
pub(crate) fn host_marker(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// 从 URL 解析 SSH `(host, port)`：`git@host:path`（22）与 `ssh://[user@]host[:port]/path`。
pub(crate) fn host_and_port(url: &str) -> Option<(String, u16)> {
    if let Some(rest) = url.strip_prefix("ssh://") {
        // IPv6 字面量 [::1]:2222
        if let Some(rest) = rest.strip_prefix('[') {
            let (host, tail) = rest.split_once(']')?;
            let port = tail
                .strip_prefix(':')
                .and_then(|p| p.split('/').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(22);
            return Some((host.to_string(), port));
        }
        let rest = rest.split_once('@').map(|(_, h)| h).unwrap_or(rest);
        let authority = rest.find(['/', '?']).map(|i| &rest[..i]).unwrap_or(rest);
        if authority.is_empty() {
            return None;
        }
        return match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                Some((h.to_string(), p.parse().ok().unwrap_or(22)))
            }
            _ => Some((authority.to_string(), 22)),
        };
    }
    // scp 形式 git@host:path（无端口语法，固定 22）
    let rest = url.strip_prefix("git@")?;
    let host = rest.split([':', '/']).next()?;
    if host.is_empty() {
        None
    } else {
        Some((host.to_string(), 22))
    }
}

/// known_hosts 校验：只认非哈希主机名（`|1|...` 哈希条目跳过，不影响放行，
/// 代价是这类主机会被当作未记录再追加一条明文条目）。
pub(crate) fn check(path: &Path, marker: &str, key_type: &str, blob: &[u8]) -> HostKeyVerdict {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HostKeyVerdict::Unknown;
    };
    let mut same_type_seen = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(host_field) = fields.next() else {
            continue;
        };
        let Some(algo) = fields.next() else { continue };
        let Some(key) = fields.next() else { continue };
        if !host_field.split(',').any(|h| h == marker) {
            continue;
        }
        if algo.eq_ignore_ascii_case(key_type) {
            same_type_seen = true;
            if b64_decode(key).is_some_and(|k| k == blob) {
                return HostKeyVerdict::Match;
            }
        }
    }
    if same_type_seen {
        HostKeyVerdict::Mismatch
    } else {
        HostKeyVerdict::Unknown
    }
}

/// 追加一行到 known_hosts（目录 / 文件不存在则创建）。
pub(crate) fn append(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_roundtrip_with_padding_edges() {
        // 51 字节（ed25519 blob 常见长度，3 的倍数）：无填充
        let blob: Vec<u8> = (0u8..=50).collect();
        let enc = b64_encode(&blob);
        assert!(!enc.contains('='));
        assert_eq!(b64_decode(&enc).unwrap(), blob);
        // 10 字节（1 mod 3）→ "=="
        let blob = vec![7u8; 10];
        let enc = b64_encode(&blob);
        assert!(enc.ends_with("=="));
        assert_eq!(b64_decode(&enc).unwrap(), blob);
        // 8 字节（2 mod 3）→ "="
        let blob = vec![9u8; 8];
        assert_eq!(b64_decode(&b64_encode(&blob)).unwrap(), blob);
        // 无填充输入（兼容部分 known_hosts 写法）
        assert_eq!(b64_decode("aGk").unwrap(), b"hi");
    }

    #[test]
    fn host_marker_and_url_parsing() {
        assert_eq!(host_marker("github.com", 22), "github.com");
        assert_eq!(host_marker("gitlab.com", 2222), "[gitlab.com]:2222");

        assert_eq!(
            host_and_port("git@github.com:hicode0101/HiGit.git").unwrap(),
            ("github.com".to_string(), 22)
        );
        assert_eq!(
            host_and_port("ssh://bob@gitlab.com:2222/user/repo.git").unwrap(),
            ("gitlab.com".to_string(), 2222)
        );
        assert_eq!(
            host_and_port("ssh://github.com/x/y.git").unwrap(),
            ("github.com".to_string(), 22)
        );
        assert!(host_and_port("https://github.com/x/y.git").is_none());
        assert!(host_and_port("D:/code/repo").is_none());
    }

    #[test]
    fn check_tofu_flow() {
        let dir = std::env::temp_dir().join(format!("higit-kh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kh = dir.join("known_hosts");
        let blob = b"hostkey-bytes-123";

        // 无文件 → 未知
        assert!(matches!(
            check(&kh, "github.com", "ssh-ed25519", blob),
            HostKeyVerdict::Unknown
        ));

        // 同算法不同密钥 → 拒绝
        let other = b64_encode(b"different-key");
        append(&kh, &format!("github.com ssh-ed25519 {other}")).unwrap();
        assert!(matches!(
            check(&kh, "github.com", "ssh-ed25519", blob),
            HostKeyVerdict::Mismatch
        ));

        // 同主机其他算法是首次出现 → 未知（OpenSSH 语义：新算法单独记录）
        assert!(matches!(
            check(&kh, "github.com", "ssh-rsa", blob),
            HostKeyVerdict::Unknown
        ));

        // 追加正确条目 → 匹配
        append(
            &kh,
            &format!("github.com,other.host ssh-ed25519 {}", b64_encode(blob)),
        )
        .unwrap();
        assert!(matches!(
            check(&kh, "github.com", "ssh-ed25519", blob),
            HostKeyVerdict::Match
        ));

        // 注释 / @cert-authority / 哈希条目不参与匹配
        append(&kh, "# comment").unwrap();
        append(&kh, "@cert-authority github.com ssh-ed25519 AAAA").unwrap();
        append(&kh, "|1|abc=|def= ssh-ed25519 AAAA").unwrap();
        assert!(matches!(
            check(&kh, "github.com", "ssh-rsa", blob),
            HostKeyVerdict::Unknown
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
