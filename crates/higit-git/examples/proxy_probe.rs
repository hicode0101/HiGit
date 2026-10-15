//! 代理诊断探针（诊断用，#[ignore] 不参与常规测试；诊断后可删除）：
//! 复现 HiGit 引擎（git2 → libgit2）在 GIT_PROXY_SPECIFIED 下的真实行为，
//! 对比不同代理协议（socks5 / socks5h / http / 无代理）能否通过认证连接。
//!
//! ```text
//! PROBE_URL=https://github.com/hicode0101/test2.git \
//! PROXY_SOCKS=socks5://127.0.0.1:10808 \
//! PROXY_SOCKSH=socks5h://127.0.0.1:10808 \
//! PROXY_HTTP=http://127.0.0.1:7891 \
//! CRED_USER=xxx CRED_PASS=yyy \
//! cargo run -p higit-git --example proxy_probe
//! ```
//!
//! 凭证只经环境变量注入，不写入任何文件。

use std::cell::Cell;

fn try_connect(label: &str, url: &str, proxy: Option<&str>, user: &str, pass: &str) {
    let repo = match git2::Repository::open(".") {
        Ok(r) => r,
        Err(e) => {
            println!("[{label}] open repo failed: {e}");
            return;
        }
    };
    let mut remote = match repo.remote_anonymous(url) {
        Ok(r) => r,
        Err(e) => {
            println!("[{label}] remote failed: {e}");
            return;
        }
    };
    let tried = Cell::new(false);
    let user = user.to_string();
    let pass = pass.to_string();
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(move |_u, _username, _allowed| {
        if tried.replace(true) {
            return Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Http,
                "probe: single attempt only",
            ));
        }
        git2::Cred::userpass_plaintext(&user, &pass)
    });
    let po = proxy.map(|p| {
        let mut po = git2::ProxyOptions::new();
        po.url(p);
        po
    });

    let started = std::time::Instant::now();
    let result = remote.connect_auth(git2::Direction::Fetch, Some(cbs), po);
    let elapsed = started.elapsed().as_millis();
    match result {
        Ok(_) => println!("[{label}] connect OK ({elapsed} ms)"),
        Err(e) => println!("[{label}] connect FAILED ({elapsed} ms): {e}"),
    }
}

fn main() {
    let url = std::env::var("PROBE_URL")
        .unwrap_or_else(|_| "https://github.com/hicode0101/test2.git".into());
    let user = std::env::var("CRED_USER").unwrap_or_default();
    let pass = std::env::var("CRED_PASS").unwrap_or_default();
    let socks = std::env::var("PROXY_SOCKS").ok();
    let socksh = std::env::var("PROXY_SOCKSH").ok();
    let http_px = std::env::var("PROXY_HTTP").ok();

    println!("== proxy probe: {url}");
    try_connect("no-proxy", &url, None, &user, &pass);
    if let Some(p) = &socks {
        try_connect("socks5  ", &url, Some(p), &user, &pass);
    }
    if let Some(p) = &socksh {
        try_connect("socks5h  ", &url, Some(p), &user, &pass);
    }
    if let Some(p) = &http_px {
        try_connect("http    ", &url, Some(p), &user, &pass);
    }
}
