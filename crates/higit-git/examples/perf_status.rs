//! 性能基准（M5，需求 §6）：N 文件仓库的 engine.status() 计时。
//!
//! 目标（需求表）：1 万文件 ≤ 1s，10 万文件 ≤ 5s。
//! 用法：cargo run --release -p higit-git --example perf_status -- [files] [runs]
//!
//! fixture：N 个文件分 500/目录，前 80% 已提交，后 20% 未跟踪；
//! status 恒含未跟踪递归（HiGit 显示层语义，docs/technical-design.md §2.3）。

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use higit_git::factory::{BackendKind, create_backend};
use higit_git::model::StatusOpts;

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let runs: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let dir = std::env::temp_dir().join(format!("higit-perf-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let t0 = Instant::now();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.init(&dir).unwrap();

    // 每 5 个文件留 1 个未跟踪并散布到所有目录：避免 libgit2 折叠全未跟踪目录，
    // 让 status 逐条枚举（比“整目录未跟踪”更接近真实混合负载）
    let is_untracked = |i: usize| i % 5 == 4;
    for i in 0..n {
        let sub = dir.join(format!("dir{:02}", i / 500));
        fs::create_dir_all(&sub).ok();
        fs::write(
            sub.join(format!("file_{i:06}.txt")),
            format!("content {i}\nsecond line for size\n"),
        )
        .unwrap();
    }
    let committed = (0..n).filter(|i| !is_untracked(*i)).count();
    let paths: Vec<PathBuf> = (0..n)
        .filter(|i| !is_untracked(*i))
        .map(|i| dir.join(format!("dir{:02}/file_{i:06}.txt", i / 500)))
        .collect();
    for chunk in paths.chunks(2000) {
        backend.stage(&session, chunk).unwrap();
    }
    backend.commit(&session, "perf fixture", None).unwrap();
    println!(
        "fixture: {n} files ({committed} committed, {} untracked) in {:.1}s",
        n - committed,
        t0.elapsed().as_secs_f64()
    );

    // 预热（索引/缓存加载），再计时取最优
    let opts = StatusOpts {
        include_ignored: true,
    };
    let warm = backend.status(&session, &opts).unwrap();
    println!("warmup: {} entries", warm.entries.len());
    let mut best = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        let st = backend.status(&session, &opts).unwrap();
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(dt);
        println!("status: {dt:.0} ms ({} entries)", st.entries.len());
    }
    let budget = if n >= 100_000 { 5000 } else { 1000 };
    let verdict = if best <= budget as f64 {
        "PASS"
    } else {
        "FAIL"
    };
    println!("RESULT N={n} best={best:.0} ms budget={budget} ms -> {verdict}");

    let _ = fs::remove_dir_all(&dir);
}
