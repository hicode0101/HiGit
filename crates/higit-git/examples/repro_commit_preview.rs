//! 复现 Tauri 命令 commit_preview 的完整调用链（含长路径降级枚举），打印统计。
//! 用法：cargo run --release -p higit-git --example repro_commit_preview -- [repo路径] [--rel 范围] [--dump 清单文件]
//! rel 范围 = 目录或单个文件的相对路径（空 = 整个仓库）；
//! --dump 把枚举的文件清单排序写出，便于与 `git ls-files --others` 逐文件比对。

use std::time::Instant;

use higit_git::factory::{create_backend, BackendKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| r"D:\AiWork\bytedance-src".into());
    let rest: Vec<String> = args.collect();
    let rel = rest
        .iter()
        .position(|a| a == "--rel")
        .and_then(|i| rest.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let dump = rest
        .iter()
        .position(|a| a == "--dump")
        .and_then(|i| rest.get(i + 1));

    let backend = create_backend(BackendKind::Git2).expect("create backend");
    let t0 = Instant::now();
    let session = match backend.open(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => {
            println!("open ERR: {e:?}");
            return;
        }
    };
    println!("open OK in {:?}", t0.elapsed());

    // 主界面同款调用：compute_status 用 StatusOpts { include_ignored: true }
    let t = Instant::now();
    match backend.status(
        &session,
        &higit_git::model::StatusOpts {
            include_ignored: true,
        },
    ) {
        Ok(s) => println!(
            "status OK: {} entries in {:?}",
            s.entries.len(),
            t.elapsed()
        ),
        Err(e) => println!("status ERR after {:?}: {e:?}", t.elapsed()),
    }

    let t = Instant::now();
    match backend.commit_preview(&session, &rel) {
        Ok(preview) => {
            println!(
                "commit_preview(rel={rel:?}) OK: {} files, long_path_count={}, samples={:?} in {:?}",
                preview.files.len(),
                preview.long_path_count,
                preview.long_path_samples,
                t.elapsed()
            );
            if let Some(dump) = dump {
                let mut paths: Vec<&str> = preview.files.iter().map(|f| f.path.as_str()).collect();
                paths.sort_unstable();
                std::fs::write(dump, paths.join("\n") + "\n").unwrap();
                println!("dumped {} paths -> {dump}", paths.len());
            }
        }
        Err(e) => println!("commit_preview ERR after {:?}: {e:?}", t.elapsed()),
    }
}
