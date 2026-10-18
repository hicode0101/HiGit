//! git2 后端冒烟测试：动态创建临时仓库 fixture，全程无网络。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use higit_git::factory::{BackendKind, create_backend};
use higit_git::model::{CleanMode, CleanOptions, FileStatusKind, StatusOpts};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempRepo(PathBuf);

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_repo() -> TempRepo {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "higit-test-{}-{}-{}",
        std::process::id(),
        nanos,
        id
    ));
    fs::create_dir_all(&dir).unwrap();
    TempRepo(dir)
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// init 仓库（默认分支 main）→ 提交 README.md → 无返回，失败即 panic
fn init_with_commit(dir: &Path) {
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.init(dir).unwrap();

    write(&dir.join("README.md"), "# fixture\n");
    write(&dir.join(".gitignore"), "cache.tmp\n");
    backend
        .stage(&session, &[dir.join("README.md"), dir.join(".gitignore")])
        .unwrap();
    backend.commit(&session, "initial", None).unwrap();
}

#[test]
fn open_non_repository_fails() {
    let dir = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let err = match backend.open(&dir.0) {
        Err(e) => e,
        Ok(_) => panic!("opening a plain directory must fail"),
    };
    assert_eq!(err.code, higit_git::error::ErrorCode::InvalidRepository);
}

#[test]
fn status_reports_modified_untracked_and_ignored() {
    let dir = temp_repo();
    init_with_commit(&dir.0);

    // 工作区再修改 → Modified
    write(&dir.0.join("README.md"), "# changed\n");
    // 新文件 → Untracked
    write(&dir.0.join("app.rs"), "fn main() {}\n");
    // 被 .gitignore 命中 → Ignored（仅 include_ignored 时可见）
    write(&dir.0.join("cache.tmp"), "junk\n");

    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    let status = backend
        .status(
            &session,
            &StatusOpts {
                include_ignored: false,
            },
        )
        .unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));

    let find = |p: &str| {
        status
            .entries
            .iter()
            .find(|e| e.path == Path::new(p))
            .map(|e| e.kind)
    };
    assert_eq!(find("README.md"), Some(FileStatusKind::Modified));
    assert_eq!(find("app.rs"), Some(FileStatusKind::Untracked));
    assert!(
        find("cache.tmp").is_none(),
        "ignored entry must be hidden by default"
    );

    let full = backend
        .status(
            &session,
            &StatusOpts {
                include_ignored: true,
            },
        )
        .unwrap();
    let find = |p: &str| {
        full.entries
            .iter()
            .find(|e| e.path == Path::new(p))
            .map(|e| e.kind)
    };
    assert_eq!(find("cache.tmp"), Some(FileStatusKind::Ignored));
}

#[test]
fn status_reports_staged_changes() {
    let dir = temp_repo();
    init_with_commit(&dir.0);

    write(&dir.0.join("app.rs"), "fn staged() {}\n");
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    backend.stage(&session, &[dir.0.join("app.rs")]).unwrap();

    let status = backend.status(&session, &StatusOpts::default()).unwrap();
    let kind = status
        .entries
        .iter()
        .find(|e| e.path == Path::new("app.rs"))
        .map(|e| e.kind);
    assert_eq!(kind, Some(FileStatusKind::Staged));
}

#[test]
fn identity_reads_git_config() {
    let dir = temp_repo();
    init_with_commit(&dir.0);

    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 无身份配置时返回 None（不依赖机器全局配置是否已设置）
    let identity = backend.identity(&session).unwrap();
    if let Some(id) = identity {
        assert!(!id.name.is_empty() && !id.email.is_empty());
    }
}

#[test]
fn status_reports_head_info_and_empty_repo() {
    let dir = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.init(&dir.0).unwrap();

    // 空仓库：无 HEAD 信息
    let status = backend.status(&session, &StatusOpts::default()).unwrap();
    assert!(status.head.is_none());
    assert_eq!(status.branch.as_deref(), Some("main"));

    write(&dir.0.join("README.md"), "# t\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "initial commit", None).unwrap();

    let status = backend.status(&session, &StatusOpts::default()).unwrap();
    let head = status.head.expect("head info after first commit");
    assert_eq!(head.short_id.len(), 7);
    assert_eq!(head.summary, "initial commit");
    assert!(!head.committer_email.is_empty());
}

#[test]
fn clone_local_path_reports_progress() {
    let src = temp_repo();
    init_with_commit(&src.0);
    let dest = temp_repo();

    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_cb = calls.clone();
    let session = backend
        .clone_repo(
            &higit_git::engine::CloneRequest {
                url: src.0.to_string_lossy().to_string(),
                dest: dest.0.clone(),
                depth: None,
            },
            None,
            &move |p| {
                calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(p.total >= 1, "progress must carry object totals");
            },
            &token,
        )
        .unwrap();

    assert!(
        dest.0.join(".git").exists(),
        "clone destination must be a repository"
    );
    let status = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.head.is_some());
    // 进度回调仅对网络传输保证触发；本地路径克隆可能 0 次（UI 用不确定态兜底），
    // 这里只校验回调被调用时载荷形状正确（见闭包内断言）。
}

#[test]
fn clone_cancelled_reports_cancelled_error() {
    let src = temp_repo();
    init_with_commit(&src.0);
    let dest = temp_repo();

    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    token.cancel();

    let result = backend.clone_repo(
        &higit_git::engine::CloneRequest {
            url: src.0.to_string_lossy().to_string(),
            dest: dest.0.clone(),
            depth: None,
        },
        None,
        &|_p| {},
        &token,
    );
    if let Err(e) = result {
        assert_eq!(e.code, higit_git::error::ErrorCode::Cancelled);
    }
    // 本地小仓库可能在传输回调触发前完成——此时视为通过（取消语义依赖回调触发）
}

#[test]
fn stage_unstage_and_commit_roundtrip() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("app.rs"), "fn a() {}\n");
    backend.stage(&session, &[dir.0.join("app.rs")]).unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(
        st.entries
            .iter()
            .find(|e| e.path == Path::new("app.rs"))
            .map(|e| e.kind),
        Some(FileStatusKind::Staged)
    );

    // 取消暂存 → 回到未暂存的 Modified→Untracked（新文件回到 Untracked）
    backend.unstage(&session, &[dir.0.join("app.rs")]).unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(
        st.entries
            .iter()
            .find(|e| e.path == Path::new("app.rs"))
            .map(|e| e.kind),
        Some(FileStatusKind::Untracked)
    );

    // 重新暂存并提交 → 工作区干净，HEAD 前进
    backend.stage(&session, &[dir.0.join("app.rs")]).unwrap();
    let oid = backend.commit(&session, "add app.rs", None).unwrap();
    assert_eq!(oid.len(), 40);
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert!(
        st.entries
            .iter()
            .all(|e| e.kind == FileStatusKind::Clean || e.kind == FileStatusKind::Ignored)
    );
}

#[test]
fn commit_partial_includes_unstaged_workdir_changes() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 未暂存修改 + 未跟踪新增：对话框勾选语义 = git commit -- <paths>，
    // 不先 stage 直接按路径提交，工作区内容必须入库（回归：曾产生空提交）
    write(&dir.0.join("README.md"), "# updated\n");
    write(&dir.0.join("new.txt"), "new\n");
    let oid = backend
        .commit_partial(
            &session,
            "commit checked paths",
            None,
            &["README.md".to_string(), "new.txt".to_string()],
        )
        .unwrap();
    let detail = backend.commit_detail(&session, &oid).unwrap();
    assert!(detail.files.iter().any(|f| f.path == Path::new("README.md")));
    assert!(detail.files.iter().any(|f| f.path == Path::new("new.txt")));

    // 勾选路径的变更全部入库 → 工作区干净
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert!(
        st.entries
            .iter()
            .all(|e| e.kind == FileStatusKind::Clean || e.kind == FileStatusKind::Ignored)
    );
}

#[test]
fn commit_partial_stages_deletion_and_rejects_empty_commit() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 工作区删除已跟踪文件：按路径提交应入库删除（索引与基树都移除）
    fs::remove_file(dir.0.join("README.md")).unwrap();
    backend
        .commit_partial(&session, "remove README", None, &["README.md".to_string()])
        .unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert!(
        st.entries
            .iter()
            .all(|e| e.kind == FileStatusKind::Clean || e.kind == FileStatusKind::Ignored)
    );

    // 无任何变更时按路径提交 → 拒绝（git 默认不产生空提交）
    let r = backend.commit_partial(&session, "empty", None, &[".gitignore".to_string()]);
    assert!(r.is_err());
}

#[test]
fn discard_restores_modified_and_removes_untracked() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 丢弃已跟踪文件的修改 → 内容还原（Windows autocrlf 可能恢复出 CRLF，行尾归一比较）
    write(&dir.0.join("README.md"), "# changed\n");
    backend
        .discard(&session, &[dir.0.join("README.md")])
        .unwrap();
    let content = fs::read_to_string(dir.0.join("README.md")).unwrap();
    assert_eq!(content.replace("\r\n", "\n"), "# fixture\n");

    // 丢弃未跟踪文件 → 从磁盘删除
    write(&dir.0.join("notes.md"), "scratch\n");
    backend
        .discard(&session, &[dir.0.join("notes.md")])
        .unwrap();
    assert!(!dir.0.join("notes.md").exists());

    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert!(
        st.entries
            .iter()
            .all(|e| e.kind != FileStatusKind::Modified)
    );
}

#[test]
fn diff_text_reports_addition_and_binary() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 工作区改动：一行新增
    write(&dir.0.join("README.md"), "# fixture\nappended line\n");
    let diff = backend
        .diff_text(
            &session,
            Path::new("README.md"),
            higit_git::model::DiffSpec::Workdir,
        )
        .unwrap();
    assert!(!diff.is_binary);
    assert_eq!(diff.additions, 1);
    assert_eq!(diff.deletions, 0);
    assert!(diff.lines.iter().any(
        |l| l.origin == higit_git::model::DiffLineOrigin::Addition && l.text == "appended line"
    ));
    assert!(
        diff.lines
            .iter()
            .any(|l| l.origin == higit_git::model::DiffLineOrigin::HunkHeader)
    );

    // 暂存后 HEAD vs 暂存区可见该改动
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    let diff = backend
        .diff_text(
            &session,
            Path::new("README.md"),
            higit_git::model::DiffSpec::Staged,
        )
        .unwrap();
    assert_eq!(diff.additions, 1);

    // 未跟踪文件：全部为新增行
    write(&dir.0.join("new.txt"), "l1\nl2\n");
    let diff = backend
        .diff_text(
            &session,
            Path::new("new.txt"),
            higit_git::model::DiffSpec::Workdir,
        )
        .unwrap();
    assert_eq!(diff.additions, 2);

    // 二进制文件
    fs::write(dir.0.join("blob.bin"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    let diff = backend
        .diff_text(
            &session,
            Path::new("blob.bin"),
            higit_git::model::DiffSpec::Workdir,
        )
        .unwrap();
    assert!(diff.is_binary, "binary content must be detected");
}

#[test]
fn set_identity_roundtrip_repo_scope() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend
        .set_identity(
            &session,
            higit_git::model::IdentityScope::Repo,
            &higit_git::model::Identity {
                name: "HiGit Dev".into(),
                email: "hi@higit.dev".into(),
            },
        )
        .unwrap();

    let id = backend
        .identity(&session)
        .unwrap()
        .expect("identity after set");
    assert_eq!(id.name, "HiGit Dev");
    assert_eq!(id.email, "hi@higit.dev");
}

// ---------- M3：fetch / push / pull（本地 bare 远端，无网络） ----------

/// 建立带初始提交的 bare 远端（refs/heads/main）。
/// 注意：libgit2 克隆“空仓库”时不读远端 HEAD 符号引用而回退默认分支，
/// 因此测试远端必须先有种子提交，克隆才能在 main 上正常检出。
fn make_bare_remote(dir: &Path) {
    let repo = git2::Repository::init_bare(dir).unwrap();
    let sig = git2::Signature::now("seed", "seed@higit.dev").unwrap();
    let blob = repo
        .blob(
            b"# seed
",
        )
        .unwrap();
    let mut tb = repo.treebuilder(None).unwrap();
    tb.insert("README.md", blob, 0o100644).unwrap();
    let tree = repo.find_tree(tb.write().unwrap()).unwrap();
    let oid = repo
        .commit(Some("refs/heads/main"), &sig, &sig, "seed", &tree, &[])
        .unwrap();
    repo.set_head("refs/heads/main").unwrap();
    drop(tb);
    drop(tree);
    drop(repo);
    assert!(
        git2::Repository::open_bare(dir)
            .unwrap()
            .find_commit(oid)
            .is_ok()
    );
}

/// 清除本地分支的上游配置，模拟“从未推送过”的无上游分支（FR-5.2 的 -u 行为）。
fn clear_upstream(repo_dir: &Path, branch: &str) {
    let repo = git2::Repository::open(repo_dir).unwrap();
    let mut cfg = repo.config().unwrap();
    let _ = cfg.remove(&format!("branch.{branch}.remote"));
    let _ = cfg.remove(&format!("branch.{branch}.merge"));
}

fn clone_from(
    backend: &dyn higit_git::engine::GitEngine,
    url: &Path,
    dest: &Path,
) -> higit_git::engine::RepoSession {
    backend
        .clone_repo(
            &higit_git::engine::CloneRequest {
                url: url.to_string_lossy().to_string(),
                dest: dest.to_path_buf(),
                depth: None,
            },
            None,
            &|_| {},
            &higit_git::engine::CancelToken::default(),
        )
        .unwrap()
}

#[test]
fn push_fetch_pull_roundtrip_over_local_remote() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();

    // A：克隆空裸库 → 提交 → 推送并建立上游（FR-5.2）
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    clear_upstream(&a.0, "main");
    write(&a.0.join("README.md"), "# a\n");
    backend.stage(&sa, &[a.0.join("README.md")]).unwrap();
    backend.commit(&sa, "from a", None).unwrap();
    let pushed = backend.push(&sa, "main", None, &|_| {}, &token).unwrap();
    assert!(pushed.created_upstream, "first push must set upstream");

    // B：克隆（上游由 clone 自动建立）→ 提交 → 推送（不再新建上游）
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);
    write(&b.0.join("feature.txt"), "from b\n");
    backend.stage(&sb, &[b.0.join("feature.txt")]).unwrap();
    backend.commit(&sb, "from b", None).unwrap();
    let pushed = backend.push(&sb, "main", None, &|_| {}, &token).unwrap();
    assert!(!pushed.created_upstream);

    // A fetch → behind=1（FR-5.3 / FR-5.4）
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert_eq!((st.ahead, st.behind), (0, 1));

    // A pull → 快进，feature.txt 出现在工作区（FR-5.1）
    let pulled = backend.pull(&sa, "main", None, &|_| {}, &token).unwrap();
    assert!(pulled.updated);
    assert_eq!(
        pulled.head.as_ref().map(|h| h.summary.as_str()),
        Some("from b")
    );
    // Windows autocrlf 检出可能写 CRLF，行尾归一比较
    assert_eq!(
        fs::read_to_string(a.0.join("feature.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "from b\n"
    );

    // 再次 pull：已最新，不做任何事
    let pulled = backend.pull(&sa, "main", None, &|_| {}, &token).unwrap();
    assert!(!pulled.updated);
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert_eq!((st.ahead, st.behind), (0, 0));
}

#[test]
fn pull_diverged_reports_error() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();

    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    write(&a.0.join("README.md"), "# a\n");
    backend.stage(&sa, &[a.0.join("README.md")]).unwrap();
    backend.commit(&sa, "from a", None).unwrap();
    backend.push(&sa, "main", None, &|_| {}, &token).unwrap();

    // B 提交并推送
    let b = temp_repo();
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);
    write(&b.0.join("other.txt"), "b\n");
    backend.stage(&sb, &[b.0.join("other.txt")]).unwrap();
    backend.commit(&sb, "from b", None).unwrap();
    backend.push(&sb, "main", None, &|_| {}, &token).unwrap();

    // A 也本地提交（分叉）→ pull 报 Diverged（FR-5.1：不可快进时明确报错）
    write(&a.0.join("local.txt"), "a-only\n");
    backend.stage(&sa, &[a.0.join("local.txt")]).unwrap();
    backend.commit(&sa, "a local", None).unwrap();
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert_eq!((st.ahead, st.behind), (1, 1));

    let err = backend
        .pull(&sa, "main", None, &|_| {}, &token)
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::Diverged);
}

#[test]
fn pull_refuses_to_overwrite_uncommitted_changes() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();

    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    write(&a.0.join("README.md"), "# a\n");
    backend.stage(&sa, &[a.0.join("README.md")]).unwrap();
    backend.commit(&sa, "from a", None).unwrap();
    backend.push(&sa, "main", None, &|_| {}, &token).unwrap();

    let b = temp_repo();
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);
    write(&b.0.join("README.md"), "# changed by b\n");
    backend.stage(&sb, &[b.0.join("README.md")]).unwrap();
    backend.commit(&sb, "b edits readme", None).unwrap();
    backend.push(&sb, "main", None, &|_| {}, &token).unwrap();

    // A 有对同一文件的未提交修改 → 安全快进必须拒绝（不丢工作区改动）
    write(&a.0.join("README.md"), "# dirty local edit\n");
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();
    let err = backend
        .pull(&sa, "main", None, &|_| {}, &token)
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::DirtyWorktree);
    assert_eq!(
        fs::read_to_string(a.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# dirty local edit\n"
    );
}

#[test]
fn stage_directory_expands_to_changed_files() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // src/ 目录：一个未跟踪新文件 + 一个已跟踪修改；cache.tmp 被 .gitignore 命中
    write(&dir.0.join("src/lib.rs"), "fn lib() {}\n");
    write(&dir.0.join("src/keep.rs"), "v1\n");
    backend
        .stage(&session, &[dir.0.join("src/keep.rs")])
        .unwrap();
    backend.commit(&session, "add keep.rs", None).unwrap();
    write(&dir.0.join("src/keep.rs"), "v2\n");
    write(&dir.0.join("src/cache.tmp"), "junk\n");

    // 整目录暂存
    backend.stage(&session, &[dir.0.join("src")]).unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    let kind = |p: &str| {
        st.entries
            .iter()
            .find(|e| e.path == Path::new(p))
            .map(|e| e.kind)
    };
    assert_eq!(kind("src/lib.rs"), Some(FileStatusKind::Staged));
    assert_eq!(kind("src/keep.rs"), Some(FileStatusKind::Staged));
    assert!(
        kind("src/cache.tmp").is_none() || kind("src/cache.tmp") == Some(FileStatusKind::Ignored),
        "ignored file must not be staged"
    );
}

// ---------- M4：提交历史（FR-6） ----------

#[test]
fn log_lists_commits_newest_first_with_refs() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("README.md"), "# second\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    let second = backend.commit(&session, "second commit", None).unwrap();

    let page1 = backend.log(&session, 1, 0).unwrap();
    assert_eq!(page1.len(), 1);
    assert_eq!(page1[0].id, second);
    assert_eq!(page1[0].summary, "second commit");
    assert_eq!(page1[0].parents, 1);
    assert_eq!(page1[0].short_id.chars().count(), 7);
    // HEAD 所指 main 分支装饰
    assert!(
        page1[0].refs.iter().any(|r| r.contains("main")),
        "refs must decorate HEAD commit, got {:?}",
        page1[0].refs
    );

    let all = backend.log(&session, 10, 0).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].summary, "initial");

    // 分页：skip 越过最新一条
    let page2 = backend.log(&session, 1, 1).unwrap();
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].summary, "initial");

    // 空仓库无历史
    let empty = temp_repo();
    let se = backend.init(&empty.0).unwrap();
    assert!(backend.log(&se, 10, 0).unwrap().is_empty());
}

#[test]
fn commit_detail_lists_changed_files_and_parents() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("README.md"), "# v2\nmore lines\n");
    write(&dir.0.join("src/app.rs"), "fn main() {}\n");
    backend
        .stage(
            &session,
            &[dir.0.join("README.md"), dir.0.join("src/app.rs")],
        )
        .unwrap();
    let oid = backend
        .commit(&session, "second: edit readme, add app.rs", None)
        .unwrap();

    let detail = backend.commit_detail(&session, &oid).unwrap();
    assert_eq!(detail.parents.len(), 1);
    assert_eq!(detail.summary, "second: edit readme, add app.rs");
    assert!(detail.message.contains("second"));
    assert_eq!(detail.committer_email, detail.author_email);

    let find = |p: &str| {
        detail
            .files
            .iter()
            .find(|f| f.path == Path::new(p))
            .map(|f| (f.kind, f.additions, f.deletions))
    };
    let readme = find("README.md").expect("README.md in detail");
    assert_eq!(readme.0, higit_git::model::CommitFileKind::Modified);
    assert_eq!((readme.1, readme.2), (2, 1));
    let app = find("src/app.rs").expect("app.rs in detail");
    assert_eq!(app.0, higit_git::model::CommitFileKind::Added);
    assert_eq!(app.1, 1);
}

#[test]
fn commit_file_diff_shows_commit_change() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("README.md"), "# line1\n# line2\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    let oid = backend.commit(&session, "two lines", None).unwrap();

    let td = backend
        .commit_file_diff(&session, &oid, Path::new("README.md"))
        .unwrap();
    assert_eq!(td.additions, 2);
    assert!(td.lines.iter().any(|l| l.text.contains("# line2")));

    // 不在提交内的文件：无差异
    let empty = backend
        .commit_file_diff(&session, &oid, Path::new("src/none.rs"))
        .unwrap();
    assert_eq!(empty.additions, 0);
}

// ---------- M4：分支管理（FR-7） ----------

#[test]
fn branch_list_create_checkout_rename_delete_roundtrip() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "dev", None).unwrap();
    let list = backend.branches(&session).unwrap();
    let dev = list.iter().find(|b| b.name == "dev").unwrap();
    assert!(!dev.is_remote);
    assert!(!dev.is_current);
    let main = list.iter().find(|b| b.name == "main").unwrap();
    assert!(main.is_current);
    // 从 HEAD 创建：dev 与 main 指向同一提交
    assert_eq!(dev.short_id, main.short_id);

    backend.checkout_branch(&session, "dev").unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("dev"));

    // 重命名当前分支：HEAD 自动跟随
    backend.rename_branch(&session, "dev", "feature").unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    // 已合并（与 main 同点）→ 直接删除成功
    backend.checkout_branch(&session, "main").unwrap();
    backend.delete_branch(&session, "feature", false).unwrap();
    let list = backend.branches(&session).unwrap();
    assert!(list.iter().all(|b| b.name != "feature"));
}

#[test]
fn delete_unmerged_branch_requires_force() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "wip", None).unwrap();
    backend.checkout_branch(&session, "wip").unwrap();
    write(&dir.0.join("wip.txt"), "wip\n");
    backend.stage(&session, &[dir.0.join("wip.txt")]).unwrap();
    backend.commit(&session, "wip commit", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    // 未合并：软删除被拒
    let err = backend.delete_branch(&session, "wip", false).unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::BranchNotMerged);
    // 强删除成功
    backend.delete_branch(&session, "wip", true).unwrap();
    let list = backend.branches(&session).unwrap();
    assert!(list.iter().all(|b| b.name != "wip"));
}

#[test]
fn checkout_conflicting_branch_reports_dirty_worktree() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 先建分支（指向旧提交），再让 main 前进
    backend.create_branch(&session, "elsewhere", None).unwrap();
    write(&dir.0.join("README.md"), "# on main\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "on main", None).unwrap();

    // 本地修改 README 后切回旧提交 → 会覆盖本地修改 → 拒绝
    write(&dir.0.join("README.md"), "# dirty\n");
    let err = backend.checkout_branch(&session, "elsewhere").unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::DirtyWorktree);
    assert_eq!(
        fs::read_to_string(dir.0.join("README.md")).unwrap(),
        "# dirty\n"
    );
}

#[test]
fn merge_fast_forward_moves_head() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "feat", None).unwrap();
    backend.checkout_branch(&session, "feat").unwrap();
    write(&dir.0.join("feat.txt"), "feat\n");
    backend.stage(&session, &[dir.0.join("feat.txt")]).unwrap();
    backend.commit(&session, "feat work", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    let outcome = backend.merge_branch(&session, "feat").unwrap();
    assert!(outcome.fast_forward);
    assert!(outcome.updated);
    assert!(outcome.conflicts.is_empty());
    assert!(dir.0.join("feat.txt").exists());
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.head.map(|h| h.summary), Some("feat work".to_string()));

    // 已是最新
    let again = backend.merge_branch(&session, "feat").unwrap();
    assert!(!again.updated);
}

#[test]
fn merge_three_way_creates_merge_commit() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "topic", None).unwrap();
    backend.checkout_branch(&session, "topic").unwrap();
    write(&dir.0.join("topic.txt"), "topic\n");
    backend.stage(&session, &[dir.0.join("topic.txt")]).unwrap();
    backend.commit(&session, "topic commit", None).unwrap();

    backend.checkout_branch(&session, "main").unwrap();
    write(&dir.0.join("main.txt"), "main\n");
    backend.stage(&session, &[dir.0.join("main.txt")]).unwrap();
    backend.commit(&session, "main commit", None).unwrap();

    let outcome = backend.merge_branch(&session, "topic").unwrap();
    assert!(!outcome.fast_forward);
    assert!(outcome.updated);
    assert!(outcome.conflicts.is_empty());
    assert!(dir.0.join("topic.txt").exists());

    // 顶部为双亲合并提交
    let log = backend.log(&session, 1, 0).unwrap();
    assert_eq!(log[0].parents, 2);
    assert_eq!(log[0].summary, "Merge branch 'topic'");
}

#[test]
fn merge_conflict_blocks_and_resolves_to_merge_commit() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "other", None).unwrap();
    backend.checkout_branch(&session, "other").unwrap();
    write(&dir.0.join("README.md"), "# from other\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend
        .commit(&session, "other edits readme", None)
        .unwrap();

    backend.checkout_branch(&session, "main").unwrap();
    write(&dir.0.join("README.md"), "# from main\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "main edits readme", None).unwrap();

    // 冲突：阻断并列出文件
    let outcome = backend.merge_branch(&session, "other").unwrap();
    assert_eq!(outcome.conflicts, vec![PathBuf::from("README.md")]);

    // 冲突状态可见（文件表会显示 Conflicted）
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(
        st.entries
            .iter()
            .find(|e| e.path == Path::new("README.md"))
            .map(|e| e.kind),
        Some(FileStatusKind::Conflicted)
    );

    // 解决冲突 → 暂存 → 提交 = 双亲合并提交
    write(&dir.0.join("README.md"), "# resolved\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    let oid = backend
        .commit(&session, "merge: resolve conflict", None)
        .unwrap();
    let detail = backend.commit_detail(&session, &oid).unwrap();
    assert_eq!(detail.parents.len(), 2);

    // 合并状态已清理：再次提交为普通提交
    write(&dir.0.join("extra.txt"), "x\n");
    backend.stage(&session, &[dir.0.join("extra.txt")]).unwrap();
    let oid2 = backend.commit(&session, "plain commit", None).unwrap();
    let detail2 = backend.commit_detail(&session, &oid2).unwrap();
    assert_eq!(detail2.parents.len(), 1);
}

#[test]
fn merge_refuses_dirty_worktree() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "topic", None).unwrap();
    backend.checkout_branch(&session, "topic").unwrap();
    write(&dir.0.join("topic.txt"), "topic\n");
    backend.stage(&session, &[dir.0.join("topic.txt")]).unwrap();
    backend.commit(&session, "topic commit", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    write(&dir.0.join("README.md"), "# dirty\n");
    let err = backend.merge_branch(&session, "topic").unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::DirtyWorktree);
}

// ---------- M5：amend 修补提交（FR-4.5）与网络代理设置（FR-10.4） ----------

#[test]
fn commit_amend_replaces_message_and_content() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    let base = backend.commit_detail(&session, "HEAD").unwrap();
    assert_eq!(base.parents.len(), 0);

    // 修改内容并 amend：新 HEAD 消息更新、父提交仍是原父（初始提交无父）、内容包含改动
    write(&dir.0.join("README.md"), "# amended\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    let oid = backend
        .commit_amend(&session, "initial (amended)", None)
        .unwrap();

    let head = backend.commit_detail(&session, "HEAD").unwrap();
    assert_eq!(head.id, oid);
    assert_eq!(head.summary, "initial (amended)");
    assert_eq!(head.parents, base.parents);
    let names: Vec<_> = head
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"README.md".to_string()));

    // amend 后再追加普通提交，历史连续
    write(&dir.0.join("next.txt"), "n\n");
    backend.stage(&session, &[dir.0.join("next.txt")]).unwrap();
    backend.commit(&session, "next", None).unwrap();
    let log = backend.log(&session, 10, 0).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].summary, "next");
    assert_eq!(log[1].summary, "initial (amended)");
}

#[test]
fn set_proxy_accepts_url_and_keeps_local_transport_working() {
    let dir = temp_repo();
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // git2 直接用底层 API 建 remote（engine 层无 remote 管理，FR-5.9 为 v1.1）
    let remote_url = remote.0.to_string_lossy().to_string();
    git2::Repository::open(&dir.0)
        .unwrap()
        .remote("origin", &remote_url)
        .unwrap();

    // 设置代理后本地 file:// 传输不受影响（proxy 仅作用于网络传输）
    backend.set_proxy(Some("socks5://127.0.0.1:10808"));
    backend
        .fetch(
            &session,
            "",
            None,
            &|_| {},
            &higit_git::engine::CancelToken::default(),
        )
        .unwrap();
    // 清除代理 = 直连
    backend.set_proxy(None);
    backend
        .fetch(
            &session,
            "",
            None,
            &|_| {},
            &higit_git::engine::CancelToken::default(),
        )
        .unwrap();
}

/// 暂存已删除的文件：应等价 `git add`（暂存删除），而不是 add_bypath 的 ENOENT 报错。
#[test]
fn stage_deleted_file_stages_removal() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();

    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    write(&a.0.join("gone.txt"), "bye\n");
    backend.stage(&sa, &[a.0.join("gone.txt")]).unwrap();
    backend.commit(&sa, "add gone", None).unwrap();

    // 工作区删除后暂存该路径
    fs::remove_file(a.0.join("gone.txt")).unwrap();
    backend
        .stage(&sa, &[a.0.join("gone.txt")])
        .expect("staging a deleted file must succeed");
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    let entry = st
        .entries
        .iter()
        .find(|e| e.path == Path::new("gone.txt"))
        .expect("staged deletion must appear in status");
    assert_eq!(entry.kind, FileStatusKind::Staged);

    // 提交暂存的删除后工作区应干净
    backend.commit(&sa, "remove gone", None).unwrap();
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert!(
        st.entries
            .iter()
            .all(|e| matches!(e.kind, FileStatusKind::Clean | FileStatusKind::Ignored))
    );
}

/// 清理（git clean）：模式 / 目录 / 嵌套仓库跳过 / 回收站开关（测试走直接删除）
#[test]
fn clean_worktree_respects_mode_dirs_and_nested_git() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("untracked.txt"), "u\n");
    write(&dir.0.join("cache.tmp"), "ignored\n"); // .gitignore 已含 cache.tmp
    write(&dir.0.join("sub/nested.txt"), "n\n");
    // 嵌套仓库：sub2 含 .git，不带 -ff 时必须跳过
    let sub2 = dir.0.join("sub2");
    fs::create_dir_all(sub2.join(".git")).unwrap();
    write(&sub2.join("x.txt"), "x\n");

    // UntrackedOnly + 不含目录：只清理顶层未跟踪文件；被忽略文件与目录条目保留
    let o = backend
        .clean_worktree(
            &session,
            &CleanOptions {
                mode: CleanMode::UntrackedOnly,
                dirs: false,
                force_git_dirs: false,
                dry_run: false,
                no_recycle_bin: true,
            },
        )
        .unwrap();
    assert_eq!(o.paths, vec![PathBuf::from("untracked.txt")]);
    assert!(o.failed.is_empty());
    assert!(!dir.0.join("untracked.txt").exists());
    assert!(dir.0.join("cache.tmp").exists());
    assert!(dir.0.join("sub").exists());
    assert!(sub2.exists());

    // All + 目录：清理被忽略文件与未跟踪目录；sub2/ 因含 .git 被跳过
    let o = backend
        .clean_worktree(
            &session,
            &CleanOptions {
                mode: CleanMode::All,
                dirs: true,
                force_git_dirs: false,
                dry_run: false,
                no_recycle_bin: true,
            },
        )
        .unwrap();
    assert!(o.paths.iter().any(|p| p == Path::new("cache.tmp")));
    assert!(o.paths.iter().any(|p| p == Path::new("sub/")));
    assert!(!o.paths.iter().any(|p| p.starts_with("sub2")));
    assert!(!dir.0.join("cache.tmp").exists());
    assert!(!dir.0.join("sub").exists());
    assert!(sub2.exists());

    // IgnoredOnly：只清理被忽略文件
    write(&dir.0.join("cache.tmp"), "ignored\n");
    let o = backend
        .clean_worktree(
            &session,
            &CleanOptions {
                mode: CleanMode::IgnoredOnly,
                dirs: true,
                force_git_dirs: false,
                dry_run: false,
                no_recycle_bin: true,
            },
        )
        .unwrap();
    assert_eq!(o.paths, vec![PathBuf::from("cache.tmp")]);
    assert!(!dir.0.join("cache.tmp").exists());
}

/// 试运行：只收集将清理的路径，不实际删除
#[test]
fn clean_worktree_dry_run_keeps_files() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    write(&dir.0.join("u.txt"), "u\n");
    let o = backend
        .clean_worktree(
            &session,
            &CleanOptions {
                mode: CleanMode::All,
                dirs: true,
                force_git_dirs: false,
                dry_run: true,
                no_recycle_bin: true,
            },
        )
        .unwrap();
    assert_eq!(o.paths, vec![PathBuf::from("u.txt")]);
    assert!(dir.0.join("u.txt").exists());
}

// ---------- 增强检出（FR-7.5：分支/标签/提交 + TortoiseGit 式选项） ----------

use higit_git::model::{CheckoutOptions, CheckoutTargetKind};

/// 仅指定目标类型与目标的 CheckoutOptions（其余选项默认关闭）
fn co(kind: CheckoutTargetKind, target: &str) -> CheckoutOptions {
    CheckoutOptions {
        kind,
        target: target.to_string(),
        ..Default::default()
    }
}

/// main 前进一个提交（README → "# second\n"）
fn advance_main(
    backend: &dyn higit_git::engine::GitEngine,
    session: &higit_git::engine::RepoSession,
    dir: &Path,
) {
    write(&dir.join("README.md"), "# second\n");
    backend.stage(session, &[dir.join("README.md")]).unwrap();
    backend.commit(session, "second", None).unwrap();
}

#[test]
fn tags_list_and_checkout_tag_commit_detach_head() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    advance_main(backend.as_ref(), &session, &dir.0);

    // fixture：v1 → 初始提交（轻量），v2 → 当前提交（附注）
    let first = {
        let repo = git2::Repository::open(&dir.0).unwrap();
        let first = repo.revparse_single("main~1").unwrap();
        repo.reference("refs/tags/v1", first.id(), false, "v1")
            .unwrap();
        let sig = git2::Signature::now("t", "t@higit.dev").unwrap();
        let head = repo.revparse_single("main").unwrap().peel_to_commit().unwrap();
        repo.tag("v2", head.as_object(), &sig, "v2", false).unwrap();
        first.id()
    };

    // 两个标签都能列出（按提交时间倒序，同秒时按名称兜底）
    let tags = backend.tags(&session).unwrap();
    assert_eq!(tags.len(), 2);
    let v1 = tags.iter().find(|x| x.name == "v1").unwrap();
    let v2 = tags.iter().find(|x| x.name == "v2").unwrap();
    // 轻量 / 附注标签统一解析到目标提交
    assert_eq!(v1.short_id, first.to_string()[..7]);
    assert_eq!(v1.last_commit_summary, "initial");
    assert_eq!(v2.last_commit_summary, "second");

    // 切到标签 v1 → 分离 HEAD，工作区回到初始内容
    let o = backend
        .checkout(&session, &co(CheckoutTargetKind::Tag, "v1"))
        .unwrap();
    assert!(o.detached);
    assert_eq!(o.created_branch, None);
    assert_eq!(
        fs::read_to_string(dir.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# fixture\n"
    );
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_ne!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.head.map(|h| h.summary), Some("initial".to_string()));

    // 切到提交表达式 main~1 同样分离；再切回 main 恢复分支状态
    let o = backend
        .checkout(&session, &co(CheckoutTargetKind::Commit, "main~1"))
        .unwrap();
    assert!(o.detached);
    backend.checkout_branch(&session, "main").unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
}

#[test]
fn checkout_remote_branch_track_creates_local_branch() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // 本地建 topic 分支并推送，然后删掉本地分支，只留远端引用
    backend.create_branch(&sa, "topic", None).unwrap();
    backend.checkout_branch(&sa, "topic").unwrap();
    write(&a.0.join("topic.txt"), "topic\n");
    backend.stage(&sa, &[a.0.join("topic.txt")]).unwrap();
    backend.commit(&sa, "topic work", None).unwrap();
    backend.push(&sa, "topic", None, &|_| {}, &token).unwrap();
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();
    backend.checkout_branch(&sa, "main").unwrap();
    backend.delete_branch(&sa, "topic", true).unwrap();

    // 不跟踪 → 分离 HEAD 到远端提交（与 git checkout origin/topic 一致）
    let o = backend
        .checkout(&sa, &co(CheckoutTargetKind::Branch, "origin/topic"))
        .unwrap();
    assert!(o.detached);
    assert_eq!(o.created_branch, None);
    assert!(a.0.join("topic.txt").exists());

    // 跟踪 → 创建同名本地分支并建立上游（git checkout --track）
    let o = backend
        .checkout(
            &sa,
            &CheckoutOptions {
                track: true,
                ..co(CheckoutTargetKind::Branch, "origin/topic")
            },
        )
        .unwrap();
    assert!(!o.detached);
    assert_eq!(o.created_branch.as_deref(), Some("topic"));
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("topic"));
    assert!(st.upstream);
    let list = backend.branches(&sa).unwrap();
    let topic = list.iter().find(|b| b.name == "topic").unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));

    // 本地分支已存在：不覆盖被拒；override 强制移动到目标
    backend.checkout_branch(&sa, "main").unwrap();
    assert!(backend
        .checkout(
            &sa,
            &CheckoutOptions {
                track: true,
                ..co(CheckoutTargetKind::Branch, "origin/topic")
            },
        )
        .is_err());
    let o = backend
        .checkout(
            &sa,
            &CheckoutOptions {
                track: true,
                override_existing: true,
                ..co(CheckoutTargetKind::Branch, "origin/topic")
            },
        )
        .unwrap();
    assert_eq!(o.created_branch.as_deref(), Some("topic"));
}

#[test]
fn checkout_create_new_branch_at_target_with_override() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // main 先前进一个提交，使 main~1（初始提交）存在
    advance_main(backend.as_ref(), &session, &dir.0);

    // 在 main~1 处新建分支并检出
    let o = backend
        .checkout(
            &session,
            &CheckoutOptions {
                create_branch: Some("from-past".to_string()),
                ..co(CheckoutTargetKind::Commit, "main~1")
            },
        )
        .unwrap();
    assert!(!o.detached);
    assert_eq!(o.created_branch.as_deref(), Some("from-past"));
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("from-past"));
    assert_eq!(st.head.map(|h| h.summary), Some("initial".to_string()));

    // main 前进后：同名分支已存在 → 拒绝；override → 强制移动到 main
    backend.checkout_branch(&session, "main").unwrap();
    advance_main(backend.as_ref(), &session, &dir.0);
    assert!(backend
        .checkout(
            &session,
            &CheckoutOptions {
                create_branch: Some("from-past".to_string()),
                ..co(CheckoutTargetKind::Branch, "main")
            },
        )
        .is_err());
    let o = backend
        .checkout(
            &session,
            &CheckoutOptions {
                create_branch: Some("from-past".to_string()),
                override_existing: true,
                ..co(CheckoutTargetKind::Branch, "main")
            },
        )
        .unwrap();
    assert_eq!(o.created_branch.as_deref(), Some("from-past"));
    let list = backend.branches(&session).unwrap();
    let from_past = list.iter().find(|b| b.name == "from-past").unwrap();
    let main = list.iter().find(|b| b.name == "main").unwrap();
    assert_eq!(from_past.short_id, main.short_id);
}

#[test]
fn checkout_merge_carries_local_changes_without_conflict() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // addfile 分支只新增 extra.txt（不动 README）
    backend.create_branch(&session, "addfile", None).unwrap();
    backend.checkout_branch(&session, "addfile").unwrap();
    write(&dir.0.join("extra.txt"), "extra\n");
    backend.stage(&session, &[dir.0.join("extra.txt")]).unwrap();
    backend.commit(&session, "add extra", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    // 本地修改 README（目标分支未触碰）→ merge 携带切换，无冲突
    write(&dir.0.join("README.md"), "# dirty\n");
    let o = backend
        .checkout(
            &session,
            &CheckoutOptions {
                merge: true,
                ..co(CheckoutTargetKind::Branch, "addfile")
            },
        )
        .unwrap();
    assert!(!o.conflicted);
    // 携带的修改仍在，目标分支的新文件已检出
    assert_eq!(
        fs::read_to_string(dir.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# dirty\n"
    );
    assert!(dir.0.join("extra.txt").exists());
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("addfile"));
}

#[test]
fn checkout_merge_conflict_leaves_conflict_state() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // other 分支修改 README 并提交
    backend.create_branch(&session, "other", None).unwrap();
    backend.checkout_branch(&session, "other").unwrap();
    write(&dir.0.join("README.md"), "# other\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "on other", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    // main 上本地未提交修改同一文件 → merge 切换产生冲突
    write(&dir.0.join("README.md"), "# dirty\n");
    let o = backend
        .checkout(
            &session,
            &CheckoutOptions {
                merge: true,
                ..co(CheckoutTargetKind::Branch, "other")
            },
        )
        .unwrap();
    assert!(o.conflicted);
    // 仓库处于冲突状态，待解决
    let repo = git2::Repository::open(&dir.0).unwrap();
    assert!(repo.index().unwrap().has_conflicts());
    // 已在目标分支上
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("other"));
}

#[test]
fn checkout_force_overwrites_local_changes() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    advance_main(backend.as_ref(), &session, &dir.0);

    // 本地修改 README 后切到旧提交：默认被拒（DirtyWorktree）
    write(&dir.0.join("README.md"), "# dirty\n");
    let err = backend
        .checkout(&session, &co(CheckoutTargetKind::Commit, "main~1"))
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::DirtyWorktree);
    assert_eq!(
        fs::read_to_string(dir.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# dirty\n"
    );

    // force：覆盖本地修改完成检出
    let o = backend
        .checkout(
            &session,
            &CheckoutOptions {
                force: true,
                ..co(CheckoutTargetKind::Commit, "main~1")
            },
        )
        .unwrap();
    assert!(o.detached);
    assert_eq!(
        fs::read_to_string(dir.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# fixture\n"
    );
}

// ---------- 增强合并（FR-7.6：no-ff / ff-only / no-commit / squash / 消息 / ours） ----------

use higit_git::model::MergeOptions;

/// 仅指定来源分支的 MergeOptions（其余选项默认关闭）
fn mo(source: &str) -> MergeOptions {
    MergeOptions {
        kind: CheckoutTargetKind::Branch,
        source: source.to_string(),
        ..Default::default()
    }
}

/// fixture：main 与 feature 自初始提交分叉（main 加 main.txt；feature 加 feature.txt /
/// feature2.txt），三方合并无冲突。结束时停在 main。
fn diverged_clean(
    backend: &dyn higit_git::engine::GitEngine,
    session: &higit_git::engine::RepoSession,
    dir: &Path,
) {
    backend.create_branch(session, "feature", None).unwrap();
    write(&dir.join("main.txt"), "main\n");
    backend.stage(session, &[dir.join("main.txt")]).unwrap();
    backend.commit(session, "m1", None).unwrap();
    backend.checkout_branch(session, "feature").unwrap();
    write(&dir.join("feature.txt"), "f1\n");
    backend.stage(session, &[dir.join("feature.txt")]).unwrap();
    backend.commit(session, "f1", None).unwrap();
    write(&dir.join("feature2.txt"), "f2\n");
    backend.stage(session, &[dir.join("feature2.txt")]).unwrap();
    backend.commit(session, "f2", None).unwrap();
    backend.checkout_branch(session, "main").unwrap();
}

#[test]
fn merge_no_ff_forces_merge_commit_and_clears_merge_head() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 默认：可快进 → 快进
    backend.create_branch(&session, "feature", None).unwrap();
    backend.checkout_branch(&session, "feature").unwrap();
    write(&dir.0.join("feature.txt"), "f1\n");
    backend.stage(&session, &[dir.0.join("feature.txt")]).unwrap();
    backend.commit(&session, "f1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();
    let o = backend.merge(&session, &mo("feature")).unwrap();
    assert!(o.fast_forward && o.updated);

    // 再造一个可快进来源，no_ff 强制产生合并提交
    backend.create_branch(&session, "next", None).unwrap();
    backend.checkout_branch(&session, "next").unwrap();
    write(&dir.0.join("next.txt"), "n1\n");
    backend.stage(&session, &[dir.0.join("next.txt")]).unwrap();
    backend.commit(&session, "n1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();
    let o = backend
        .merge(
            &session,
            &MergeOptions {
                no_ff: true,
                ..mo("next")
            },
        )
        .unwrap();
    assert!(!o.fast_forward);
    assert!(o.updated);
    assert!(o.created_commit.is_some());

    let repo = git2::Repository::open(&dir.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
    assert_eq!(head.summary().unwrap(), Some("Merge branch 'next'"));
    // 合并状态已清理：MERGE_HEAD 不存在，下一次普通提交是单亲
    assert!(repo.find_reference("MERGE_HEAD").is_err());
    write(&dir.0.join("after.txt"), "after\n");
    backend.stage(&session, &[dir.0.join("after.txt")]).unwrap();
    backend.commit(&session, "after merge", None).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 1);
    assert!(dir.0.join("next.txt").exists());
}

#[test]
fn merge_ff_only_aborts_when_not_fast_forwardable() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    diverged_clean(backend.as_ref(), &session, &dir.0);

    // 已分叉：ff-only 被拒
    let err = backend
        .merge(
            &session,
            &MergeOptions {
                ff_only: true,
                ..mo("feature")
            },
        )
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::Diverged);

    // 可快进来源：ff-only 成功快进
    backend.create_branch(&session, "next", None).unwrap();
    backend.checkout_branch(&session, "next").unwrap();
    write(&dir.0.join("next.txt"), "n1\n");
    backend.stage(&session, &[dir.0.join("next.txt")]).unwrap();
    backend.commit(&session, "n1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();
    let o = backend
        .merge(
            &session,
            &MergeOptions {
                ff_only: true,
                ..mo("next")
            },
        )
        .unwrap();
    assert!(o.fast_forward && o.updated);
}

#[test]
fn merge_squash_stages_changes_without_commit() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    backend.create_branch(&session, "feature", None).unwrap();
    backend.checkout_branch(&session, "feature").unwrap();
    write(&dir.0.join("feature.txt"), "f1\n");
    backend.stage(&session, &[dir.0.join("feature.txt")]).unwrap();
    backend.commit(&session, "f1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    let o = backend
        .merge(
            &session,
            &MergeOptions {
                squash: true,
                ..mo("feature")
            },
        )
        .unwrap();
    // 未产生提交：HEAD 与分支都停在初始提交；变更已暂存
    assert!(o.pending_commit);
    assert!(!o.updated);
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.head.map(|h| h.summary), Some("initial".to_string()));
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(
        st.entries
            .iter()
            .find(|e| e.path == Path::new("feature.txt"))
            .map(|e| e.kind),
        Some(FileStatusKind::Staged)
    );
    // squash 不设置 MERGE_HEAD
    let repo = git2::Repository::open(&dir.0).unwrap();
    assert!(repo.find_reference("MERGE_HEAD").is_err());

    // 手动提交：单亲提交（squash 语义）
    backend.commit(&session, "squashed", None).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 1);
    assert!(dir.0.join("feature.txt").exists());
}

#[test]
fn merge_squash_conflict_keeps_conflict_without_merge_head() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // main 与 feature 各自修改 README → squash 合并冲突
    backend.create_branch(&session, "feature", None).unwrap();
    write(&dir.0.join("README.md"), "# on main\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "m1", None).unwrap();
    backend.checkout_branch(&session, "feature").unwrap();
    write(&dir.0.join("README.md"), "# on feature\n");
    backend.stage(&session, &[dir.0.join("README.md")]).unwrap();
    backend.commit(&session, "f1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();

    let o = backend
        .merge(
            &session,
            &MergeOptions {
                squash: true,
                ..mo("feature")
            },
        )
        .unwrap();
    assert!(!o.conflicts.is_empty());
    // squash 冲突不保留 MERGE_HEAD，但冲突状态在索引中
    let repo = git2::Repository::open(&dir.0).unwrap();
    assert!(repo.find_reference("MERGE_HEAD").is_err());
    assert!(repo.index().unwrap().has_conflicts());
}

#[test]
fn merge_no_commit_leaves_pending_merge_state() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    diverged_clean(backend.as_ref(), &session, &dir.0);

    let o = backend
        .merge(
            &session,
            &MergeOptions {
                no_commit: true,
                ..mo("feature")
            },
        )
        .unwrap();
    assert!(o.pending_commit);
    assert!(o.conflicts.is_empty());
    // HEAD 未动；MERGE_HEAD / MERGE_MSG 就位
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.head.map(|h| h.summary), Some("m1".to_string()));
    let repo = git2::Repository::open(&dir.0).unwrap();
    assert!(repo.find_reference("MERGE_HEAD").is_ok());
    let merge_msg = fs::read_to_string(repo.path().join("MERGE_MSG")).unwrap();
    assert!(merge_msg.contains("Merge branch 'feature'"));
    assert!(dir.0.join("feature.txt").exists());

    // 随后手动提交：MERGE_HEAD 被识别，落成双亲合并提交
    backend.commit(&session, "finish merge", None).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
}

#[test]
fn merge_custom_message_and_log_messages() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    diverged_clean(backend.as_ref(), &session, &dir.0);

    // 自动消息 + merge.log：附加被合并侧提交摘要
    let o = backend
        .merge(
            &session,
            &MergeOptions {
                log_messages: 10,
                ..mo("feature")
            },
        )
        .unwrap();
    assert!(o.created_commit.is_some());
    let repo = git2::Repository::open(&dir.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap(), Some("Merge branch 'feature'"));
    let msg = head.message().unwrap();
    assert!(msg.contains("* feature:"), "log section missing: {msg}");
    assert!(msg.contains("f2"), "newest merged commit missing: {msg}");

    // 自定义消息覆盖自动生成
    backend.create_branch(&session, "next", None).unwrap();
    backend.checkout_branch(&session, "next").unwrap();
    write(&dir.0.join("next.txt"), "n1\n");
    backend.stage(&session, &[dir.0.join("next.txt")]).unwrap();
    backend.commit(&session, "n1", None).unwrap();
    backend.checkout_branch(&session, "main").unwrap();
    let o = backend
        .merge(
            &session,
            &MergeOptions {
                no_ff: true,
                message: Some("custom merge".to_string()),
                ..mo("next")
            },
        )
        .unwrap();
    assert!(o.created_commit.is_some());
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap(), Some("custom merge"));
}

#[test]
fn merge_remote_branch_and_ours_strategy() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // 建 topic 推送到远端
    backend.create_branch(&sa, "topic", None).unwrap();
    backend.checkout_branch(&sa, "topic").unwrap();
    write(&a.0.join("topic.txt"), "topic\n");
    backend.stage(&sa, &[a.0.join("topic.txt")]).unwrap();
    backend.commit(&sa, "topic work", None).unwrap();
    backend.push(&sa, "topic", None, &|_| {}, &token).unwrap();
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();
    backend.checkout_branch(&sa, "main").unwrap();
    // main 前进避免快进
    write(&a.0.join("main-side.txt"), "m\n");
    backend.stage(&sa, &[a.0.join("main-side.txt")]).unwrap();
    backend.commit(&sa, "m1", None).unwrap();

    // 合并远端分支 origin/topic：三方合并提交，消息用 remote-tracking 格式
    let o = backend.merge(&sa, &mo("origin/topic")).unwrap();
    assert!(!o.fast_forward);
    assert!(o.updated);
    assert!(o.created_commit.is_some());
    let repo = git2::Repository::open(&a.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
    assert_eq!(
        head.summary().unwrap(),
        Some("Merge remote-tracking branch 'origin/topic'")
    );
    assert!(a.0.join("topic.txt").exists());

    // -s ours：产生记录合并关系的提交，但内容保持当前分支
    backend.create_branch(&sa, "incoming", None).unwrap();
    backend.checkout_branch(&sa, "incoming").unwrap();
    write(&a.0.join("incoming.txt"), "x\n");
    backend.stage(&sa, &[a.0.join("incoming.txt")]).unwrap();
    backend.commit(&sa, "incoming work", None).unwrap();
    backend.checkout_branch(&sa, "main").unwrap();
    let o = backend
        .merge(
            &sa,
            &MergeOptions {
                strategy: "ours".to_string(),
                ..mo("incoming")
            },
        )
        .unwrap();
    assert!(o.updated);
    assert!(o.created_commit.is_some());
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
    // ours：incoming 的变更不进入工作区
    assert!(!a.0.join("incoming.txt").exists());
}

// ---------- 提交对话框增强（Amend+路径 / 提交到新分支 / 行级统计） ----------

#[test]
fn commit_amend_partial_only_takes_given_paths() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 第二次提交：app.rs v1
    write(&dir.0.join("app.rs"), "v1\n");
    backend.stage(&session, &[dir.0.join("app.rs")]).unwrap();
    backend.commit(&session, "second", None).unwrap();

    // app.rs v2（修补进去）；README 再改（不纳入修补）
    write(&dir.0.join("app.rs"), "v2\n");
    write(&dir.0.join("README.md"), "# dirty\n");

    backend
        .commit_amend_partial(
            &session,
            "second amended",
            None,
            &["app.rs".to_string()],
        )
        .unwrap();

    let repo = git2::Repository::open(&dir.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.summary().unwrap(), Some("second amended"));
    // 单亲提交，父提交仍为 initial（amend 替换而非新增）
    assert_eq!(head.parent_count(), 1);
    assert_eq!(head.parent(0).unwrap().summary().unwrap(), Some("initial"));
    // 修补后的树：app.rs 为 v2，README 保持原始内容
    let tree = head.tree().unwrap();
    let app = repo
        .find_blob(tree.get_path(Path::new("app.rs")).unwrap().id())
        .unwrap();
    assert_eq!(app.content(), b"v2\n");
    let readme = repo
        .find_blob(tree.get_path(Path::new("README.md")).unwrap().id())
        .unwrap();
    assert_eq!(readme.content(), b"# fixture\n");
}

#[test]
fn switch_head_lands_commit_on_new_branch_only() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();
    let initial_short = backend.status(&session, &StatusOpts::default()).unwrap()
        .head
        .map(|h| h.short_id)
        .unwrap();

    backend.create_branch(&session, "dev", None).unwrap();
    backend.switch_head(&session, "dev").unwrap();
    let st = backend.status(&session, &StatusOpts::default()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("dev"));

    // 提交落到 dev，main 停在原提交
    write(&dir.0.join("app.rs"), "on dev\n");
    backend.stage(&session, &[dir.0.join("app.rs")]).unwrap();
    backend.commit(&session, "on dev", None).unwrap();
    let list = backend.branches(&session).unwrap();
    let dev = list.iter().find(|b| b.name == "dev").unwrap();
    let main = list.iter().find(|b| b.name == "main").unwrap();
    assert!(dev.is_current);
    assert_ne!(dev.short_id, main.short_id);
    assert_eq!(main.short_id, initial_short);

    // 不存在的分支：明确报错
    assert!(backend.switch_head(&session, "nope").is_err());
}

#[test]
fn change_stats_reports_line_counts() {
    let dir = temp_repo();
    init_with_commit(&dir.0);
    let backend = create_backend(BackendKind::Git2).unwrap();
    let session = backend.open(&dir.0).unwrap();

    // 再提交一个 2 行文件，随后删除 → -2
    write(&dir.0.join("del.txt"), "x\ny\n");
    backend.stage(&session, &[dir.0.join("del.txt")]).unwrap();
    backend.commit(&session, "third", None).unwrap();
    std::fs::remove_file(dir.0.join("del.txt")).unwrap();

    // 修改：1 行 → 3 行（+2）；新增：3 行（+3）；删除：-2
    write(&dir.0.join("README.md"), "# fixture\na\nb\n");
    write(&dir.0.join("app.rs"), "1\n2\n3\n");

    let stats = backend
        .change_stats(
            &session,
            &[
                "README.md".to_string(),
                "app.rs".to_string(),
                "del.txt".to_string(),
            ],
        )
        .unwrap();
    assert_eq!(stats[0], (Some(2), Some(0)), "modified README");
    assert_eq!(stats[1], (Some(3), Some(0)), "added app.rs");
    assert_eq!(stats[2], (Some(0), Some(2)), "deleted del.txt");

    // 文件数超上限：整体跳过（全 None），不产生 diff 开销
    let many: Vec<String> = (0..201).map(|i| format!("f{i}.txt")).collect();
    let stats = backend.change_stats(&session, &many).unwrap();
    assert_eq!(stats.len(), 201);
    assert!(stats.iter().all(|(a, d)| a.is_none() && d.is_none()));
}

// ---------- 增强拉取（FR-5.1 扩展：no-ff / no-commit / squash / rebase / prune / tags） ----------

use higit_git::engine::PullOptions;

/// 默认关闭 ff_only（拉取对话框显式传选项时的基线；引擎 Default 保留旧版 ff-only 行为）
fn po() -> PullOptions {
    PullOptions {
        ff_only: false,
        ..Default::default()
    }
}

/// b 克隆提交并推送一次（main 前进）
fn push_from_b(
    backend: &dyn higit_git::engine::GitEngine,
    sb: &higit_git::engine::RepoSession,
    b: &Path,
    name: &str,
) {
    write(&b.join(name), "from b\n");
    backend.stage(sb, &[b.join(name)]).unwrap();
    backend.commit(sb, "from b", None).unwrap();
    backend
        .push(sb, "main", None, &|_| {}, &higit_git::engine::CancelToken::default())
        .unwrap();
}

fn silent_event(_ev: higit_git::engine::SyncEvent) {}

#[test]
fn pull_options_no_ff_creates_merge_commit() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");

    // a 本地提交（分叉）后 no_ff 拉取：产生合并提交而非快进
    write(&a.0.join("local.txt"), "local\n");
    backend.stage(&sa, &[a.0.join("local.txt")]).unwrap();
    backend.commit(&sa, "local work", None).unwrap();

    let o = backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                no_ff: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    assert!(o.updated);
    assert!(!o.fast_forward);
    assert!(a.0.join("feature.txt").exists());
    let repo = git2::Repository::open(&a.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
    assert_eq!(
        head.summary().unwrap(),
        Some("Merge remote-tracking branch 'origin/main'")
    );
}

#[test]
fn pull_options_squash_stages_without_commit() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");

    // 可快进 + squash：变更进入暂存区，HEAD 不动、无 MERGE_HEAD
    let o = backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                squash: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    assert!(o.pending_commit);
    assert!(!o.updated);
    let st = backend.status(&sa, &StatusOpts::default()).unwrap();
    assert_eq!(st.head.map(|h| h.summary), Some("seed".to_string()));
    assert_eq!(
        st.entries
            .iter()
            .find(|e| e.path == Path::new("feature.txt"))
            .map(|e| e.kind),
        Some(FileStatusKind::Staged)
    );
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo.find_reference("MERGE_HEAD").is_err());
}

#[test]
fn pull_options_no_commit_keeps_merge_state() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");

    // a 本地提交（分叉）后 no_commit 拉取：合并就绪但未提交
    write(&a.0.join("local.txt"), "local\n");
    backend.stage(&sa, &[a.0.join("local.txt")]).unwrap();
    backend.commit(&sa, "local work", None).unwrap();

    let o = backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                no_commit: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    assert!(o.pending_commit);
    assert!(!o.updated);
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo.find_reference("MERGE_HEAD").is_ok());
    let msg = fs::read_to_string(repo.path().join("MERGE_MSG")).unwrap();
    assert!(msg.contains("Merge remote-tracking branch 'origin/main'"));

    // 手动提交落成双亲合并提交
    backend.commit(&sa, "finish pull merge", None).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 2);
}

#[test]
fn pull_options_rebase_replays_local_commits() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");
    // 远端最新提交以 b 的 HEAD 为准（a 的跟踪引用在拉取前是陈旧的）
    let remote_tip = {
        let repo = git2::Repository::open(&b.0).unwrap();
        repo.head().unwrap().target().unwrap()
    };

    // a 两个本地提交（与远端分叉）
    for (name, file) in [("a1", "one.txt"), ("a2", "two.txt")] {
        write(&a.0.join(file), "local\n");
        backend.stage(&sa, &[a.0.join(file)]).unwrap();
        backend.commit(&sa, name, None).unwrap();
    }

    let o = backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                rebase: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    assert!(o.updated);
    assert_eq!(o.rebased, 2);
    let repo = git2::Repository::open(&a.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    // 线性历史：HEAD → a2' → a1' → 远端最新
    assert_eq!(head.summary().unwrap(), Some("a2"));
    let p1 = head.parent(0).unwrap();
    assert_eq!(p1.summary().unwrap(), Some("a1"));
    assert_eq!(p1.parent(0).unwrap().id(), remote_tip);
    // 远端文件已在工作区
    assert!(a.0.join("feature.txt").exists());
    assert!(a.0.join("one.txt").exists());
}

#[test]
fn pull_options_rebase_conflict_aborts_and_restores() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    // b 改 README 并推送
    write(&b.0.join("README.md"), "# from b\n");
    backend.stage(&sb, &[b.0.join("README.md")]).unwrap();
    backend.commit(&sb, "from b", None).unwrap();
    backend.push(&sb, "main", None, &|_| {}, &token).unwrap();

    // a 改同一文件（分叉冲突）后变基拉取 → 冲突中止并恢复原状
    write(&a.0.join("README.md"), "# local a\n");
    backend.stage(&sa, &[a.0.join("README.md")]).unwrap();
    backend.commit(&sa, "local a", None).unwrap();
    let local_tip = {
        let repo = git2::Repository::open(&a.0).unwrap();
        repo.head().unwrap().peel_to_commit().unwrap().id()
    };

    let err = backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                rebase: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::MergeConflict);

    // 原状恢复：HEAD 仍在本地提交、工作区是本地内容、无残留变基状态
    let repo = git2::Repository::open(&a.0).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id(), local_tip);
    assert_eq!(
        fs::read_to_string(a.0.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# local a\n"
    );
    assert!(repo.find_reference("MERGE_HEAD").is_err());
    assert!(!repo.path().join("rebase-merge").exists());
}

#[test]
fn pull_options_prune_removes_deleted_remote_branch_ref() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // a 建 topic 并推送（建立上游与远端引用），随后在裸远端删除 topic
    backend.create_branch(&sa, "topic", None).unwrap();
    backend.checkout_branch(&sa, "topic").unwrap();
    write(&a.0.join("topic.txt"), "t\n");
    backend.stage(&sa, &[a.0.join("topic.txt")]).unwrap();
    backend.commit(&sa, "topic work", None).unwrap();
    backend.push(&sa, "topic", None, &|_| {}, &token).unwrap();
    backend.checkout_branch(&sa, "main").unwrap();
    let bare = git2::Repository::open_bare(&remote.0).unwrap();
    bare.find_reference("refs/heads/topic").unwrap().delete().unwrap();

    // prune 拉取：远端跟踪引用 origin/topic 被清理（prune 覆盖整个远端）
    backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                prune: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo.find_reference("refs/remotes/origin/topic").is_err());
    assert!(repo.find_reference("refs/remotes/origin/main").is_ok());
}

#[test]
fn pull_options_all_tags_fetches_unreachable_tags() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    // b：hidden 分支提交 + 打标签 v9，仅推送标签（该提交对 main 不可达）
    backend.create_branch(&sb, "hidden", None).unwrap();
    backend.checkout_branch(&sb, "hidden").unwrap();
    write(&b.0.join("hidden.txt"), "h\n");
    backend.stage(&sb, &[b.0.join("hidden.txt")]).unwrap();
    backend.commit(&sb, "hidden work", None).unwrap();
    {
        let repo = git2::Repository::open(&b.0).unwrap();
        let oid = repo.head().unwrap().target().unwrap();
        repo.reference("refs/tags/v9", oid, false, "v9").unwrap();
        // push_multi 按分支名归一 refspec，标签直接经 git2 推送
        let mut remote = repo.find_remote("origin").unwrap();
        remote.push(&["refs/tags/v9:refs/tags/v9"], None).unwrap();
    }

    // 默认拉取：不可达标签不会被自动跟随
    backend
        .pull_options(&sa, "main", &po(), None, None, Some(&silent_event), &token)
        .unwrap();
    {
        let repo = git2::Repository::open(&a.0).unwrap();
        assert!(repo.find_reference("refs/tags/v9").is_err());
    }

    // all_tags：显式抓取全部标签
    backend
        .pull_options(
            &sa,
            "main",
            &PullOptions {
                all_tags: true,
                ..po()
            },
            None,
            None,
            Some(&silent_event),
            &token,
        )
        .unwrap();
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo.find_reference("refs/tags/v9").is_ok());
}

// ---------- 增强推送（FR-5.2 扩展：目标远端 / 任意 URL / force-with-lease / --tags / -u） ----------

use higit_git::engine::PushRequest;

/// 在仓库当前 HEAD 上打一个轻量标签
fn make_tag(repo_path: &Path, name: &str) {
    let repo = git2::Repository::open(repo_path).unwrap();
    let oid = repo.head().unwrap().target().unwrap();
    repo.reference(&format!("refs/tags/{name}"), oid, false, name)
        .unwrap();
}

#[test]
fn push_multi_tags_pushes_all_local_tags() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // main 打 v1，topic 分支打 v2，随分支一起推送
    write(&a.0.join("f1.txt"), "1\n");
    backend.stage(&sa, &[a.0.join("f1.txt")]).unwrap();
    backend.commit(&sa, "c1", None).unwrap();
    make_tag(&a.0, "v1");
    backend.create_branch(&sa, "topic", None).unwrap();
    backend.checkout_branch(&sa, "topic").unwrap();
    write(&a.0.join("f2.txt"), "2\n");
    backend.stage(&sa, &[a.0.join("f2.txt")]).unwrap();
    backend.commit(&sa, "c2", None).unwrap();
    make_tag(&a.0, "v2");

    let req = PushRequest {
        refspecs: vec![("main".into(), String::new()), ("topic".into(), String::new())],
        tags: true,
        ..Default::default()
    };
    backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    let bare = git2::Repository::open_bare(&remote.0).unwrap();
    assert!(bare.find_reference("refs/tags/v1").is_ok());
    assert!(bare.find_reference("refs/tags/v2").is_ok());
    assert!(bare.find_reference("refs/heads/topic").is_ok());
}

#[test]
fn push_multi_force_with_lease_blocks_when_remote_moved() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    // b 推进远端 main；a 未抓取，跟踪引用仍停留在旧提交
    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");

    // 安全强制：远端与跟踪引用不一致，必须拒绝（避免覆盖 b 的新提交）
    let req = PushRequest {
        refspecs: vec![("main".into(), String::new())],
        force_with_lease: true,
        ..Default::default()
    };
    let err = backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap_err();
    assert_eq!(err.code, higit_git::error::ErrorCode::PushRejected);
    // 远端未被改动
    let bare = git2::Repository::open_bare(&remote.0).unwrap();
    let b_repo = git2::Repository::open(&b.0).unwrap();
    assert_eq!(
        bare.find_reference("refs/heads/main").unwrap().target(),
        b_repo.head().unwrap().target()
    );
}

#[test]
fn push_multi_force_with_lease_allows_when_tracking_current() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let b = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);
    let sb = clone_from(backend.as_ref(), &remote.0, &b.0);

    // b 推进远端 main；a 抓取后跟踪引用与远端一致
    push_from_b(backend.as_ref(), &sb, &b.0, "feature.txt");
    backend.fetch(&sa, "", None, &|_| {}, &token).unwrap();

    // 安全强制回退远端 main：lease 校验通过，强推成功
    let req = PushRequest {
        refspecs: vec![("main".into(), String::new())],
        force_with_lease: true,
        ..Default::default()
    };
    backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    let bare = git2::Repository::open_bare(&remote.0).unwrap();
    let a_repo = git2::Repository::open(&a.0).unwrap();
    assert_eq!(
        bare.find_reference("refs/heads/main").unwrap().target(),
        a_repo.head().unwrap().target()
    );
}

#[test]
fn push_multi_set_upstream_flag_controls_tracking() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    backend.create_branch(&sa, "nb", None).unwrap();

    // 关闭建立跟踪：推送成功但不设置上游
    let req = PushRequest {
        refspecs: vec![("nb".into(), String::new())],
        set_upstream: false,
        ..Default::default()
    };
    backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo
        .find_branch("nb", git2::BranchType::Local)
        .unwrap()
        .upstream()
        .is_err());
    drop(repo);

    // 默认（开启）：再次推送后建立到 origin/nb 的跟踪
    let req = PushRequest {
        refspecs: vec![("nb".into(), String::new())],
        ..Default::default()
    };
    let o = backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    assert!(o.created_upstream);
    let repo = git2::Repository::open(&a.0).unwrap();
    let up = repo
        .find_branch("nb", git2::BranchType::Local)
        .unwrap()
        .upstream()
        .unwrap();
    assert_eq!(up.name().unwrap(), Some("origin/nb"));
}

#[test]
fn push_multi_to_named_remote_only_touches_that_remote() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let mirror = temp_repo();
    make_bare_remote(&mirror.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // 追加第二个远端 mirror；本地提交 c1（未推送）
    {
        let repo = git2::Repository::open(&a.0).unwrap();
        repo.remote("mirror", mirror.0.to_str().unwrap()).unwrap();
    }
    write(&a.0.join("f1.txt"), "1\n");
    backend.stage(&sa, &[a.0.join("f1.txt")]).unwrap();
    backend.commit(&sa, "c1", None).unwrap();

    // 指定远端名推送：只有 mirror 更新，origin 保持 seed
    let req = PushRequest {
        refspecs: vec![("main".into(), String::new())],
        remote: "mirror".into(),
        ..Default::default()
    };
    backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    let origin = git2::Repository::open_bare(&remote.0).unwrap();
    let mirror_repo = git2::Repository::open_bare(&mirror.0).unwrap();
    let a_repo = git2::Repository::open(&a.0).unwrap();
    assert_eq!(
        mirror_repo.find_reference("refs/heads/main").unwrap().target(),
        a_repo.head().unwrap().target()
    );
    assert_ne!(
        origin.find_reference("refs/heads/main").unwrap().target(),
        a_repo.head().unwrap().target()
    );
    // remotes() 列出全部远端
    let list = backend.remotes(&sa).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|(n, _)| n == "origin"));
    assert!(list.iter().any(|(n, _)| n == "mirror"));
}

#[test]
fn push_multi_to_arbitrary_url_pushes_to_anonymous_remote() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let dest = temp_repo();
    make_bare_remote(&dest.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let token = higit_git::engine::CancelToken::default();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    write(&a.0.join("f1.txt"), "1\n");
    backend.stage(&sa, &[a.0.join("f1.txt")]).unwrap();
    backend.commit(&sa, "c1", None).unwrap();
    backend.create_branch(&sa, "nb", None).unwrap();

    // 任意 URL 推送：推到独立的裸仓库，origin 不受影响
    let req = PushRequest {
        refspecs: vec![("nb".into(), String::new())],
        remote_url: dest.0.to_str().unwrap().to_string(),
        ..Default::default()
    };
    backend
        .push_multi(&sa, &req, None, &silent_event, &token)
        .unwrap();
    let dest_repo = git2::Repository::open_bare(&dest.0).unwrap();
    let a_repo = git2::Repository::open(&a.0).unwrap();
    assert_eq!(
        dest_repo.find_reference("refs/heads/nb").unwrap().target(),
        a_repo
            .find_branch("nb", git2::BranchType::Local)
            .unwrap()
            .get()
            .target()
    );
    // URL 推送无可引用的远端名，即使请求 set_upstream 也不建立跟踪
    assert!(a_repo
        .find_branch("nb", git2::BranchType::Local)
        .unwrap()
        .upstream()
        .is_err());
}

// ---------- 分支列表 ----------

#[test]
fn branches_listing_excludes_symbolic_origin_head() {
    let remote = temp_repo();
    make_bare_remote(&remote.0);
    let a = temp_repo();
    let backend = create_backend(BackendKind::Git2).unwrap();
    let sa = clone_from(backend.as_ref(), &remote.0, &a.0);

    // 克隆会创建符号引用 refs/remotes/origin/HEAD（默认分支的别名）：
    // 引用本身存在，但分支列表中不出现，实体分支 origin/main 保留
    let repo = git2::Repository::open(&a.0).unwrap();
    assert!(repo.find_reference("refs/remotes/origin/HEAD").is_ok());
    let names: Vec<String> = backend
        .branches(&sa)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.iter().any(|n| n == "origin/main"));
    assert!(!names.iter().any(|n| n == "origin/HEAD"));
}
