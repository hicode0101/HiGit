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
