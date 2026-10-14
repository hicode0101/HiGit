//! SSH 认证探针（诊断用，#[ignore]）：对比不同凭证方式能否通过远端认证。
//! 默认测试不会运行；显式执行：
//!
//! ```text
//! PROBE_KEY=D:/RSA_DSA/github_hicode0101_ed25519_256 \
//! cargo test -p higit-git --test ssh_probe -- --ignored --nocapture
//! ```
//!
//! 只做 connect + 列出远端引用（只读认证），不拉取数据。

use std::path::Path;

/// 每个策略独立会话：构造成功但服务端拒绝时返回 Err（不重试，避免链路空转）。
fn connect_once(url: &str, cbs: git2::RemoteCallbacks) -> Result<usize, String> {
    let repo = git2::Repository::discover(".").map_err(|e| e.to_string())?;
    let mut remote = repo.remote_anonymous(url).map_err(|e| e.to_string())?;
    remote
        .connect_auth(git2::Direction::Fetch, Some(cbs), None)
        .map_err(|e| e.to_string())?;
    let refs = remote.list().map_err(|e| e.to_string())?;
    Ok(refs.len())
}

/// 单次尝试凭证：第二次回调（服务端拒绝后重新询问）立即报错结束。
fn single_shot<F>(make_cred: F) -> git2::RemoteCallbacks<'static>
where
    F: for<'a> Fn(Option<&'a str>) -> Result<git2::Cred, git2::Error> + Send + 'static,
{
    let tried = std::cell::Cell::new(false);
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(move |_url, user, _allowed| {
        if tried.replace(true) {
            return Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Ssh,
                "probe: single attempt only",
            ));
        }
        make_cred(user)
    });
    cbs
}

#[test]
#[ignore = "诊断用：需要网络与真实密钥路径（PROBE_KEY 环境变量）"]
fn probe_ssh_credential_strategies() {
    let url = std::env::var("PROBE_URL").unwrap_or("git@github.com:hicode0101/test1.git".to_string());
    let key = std::env::var("PROBE_KEY").expect("PROBE_KEY 未设置");

    // 1) 与应用 Configured 步一致：仅私钥，无 .pub、无口令
    let k = key.clone();
    let r = connect_once(
        &url,
        single_shot(move |user| {
            git2::Cred::ssh_key(user.unwrap_or("git"), None, Path::new(&k), None)
        }),
    );
    println!("[1] 仅私钥（无 .pub、无口令）: {:?}", r);

    // 2) 私钥 + 显式 .pub 公钥文件
    let pubk = format!("{key}.pub");
    if Path::new(&pubk).exists() {
        let k = key.clone();
        let r = connect_once(
            &url,
            single_shot(move |user| {
                git2::Cred::ssh_key(
                    user.unwrap_or("git"),
                    Some(Path::new(&pubk)),
                    Path::new(&k),
                    None,
                )
            }),
        );
        println!("[2] 私钥 + .pub 公钥: {:?}", r);
    } else {
        println!("[2] 私钥 + .pub 公钥: 跳过（{pubk} 不存在）");
    }

    // 3) ssh-agent（Windows：OpenSSH agent 管道 / Pageant）
    let r = connect_once(
        &url,
        single_shot(|user| git2::Cred::ssh_key_from_agent(user.unwrap_or("git"))),
    );
    println!("[3] ssh-agent: {:?}", r);
}
