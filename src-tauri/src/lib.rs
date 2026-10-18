//! HiGit（Tauri 2 版）：Rust 后端复用 higit-git / higit-core，UI 为 Vue 3 前端。

// CommandError 为小结构体但经 60+ 命令的 Result 返回，clippy result_large_err 噪音过大，统一放行
#![allow(clippy::result_large_err)]

mod commands;
mod dto;
mod error;
mod platform;
mod state;

use higit_git::factory::{BackendKind, create_backend};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (config, _corrupted) = state::load_config();
    // git2 后端（CR-1：git2 类型不越出 higit-git）
    let engine: std::sync::Arc<dyn higit_git::engine::GitEngine> =
        std::sync::Arc::from(create_backend(BackendKind::Git2).expect("git2 backend"));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(state::AppState::new(engine, config))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_repo_settings,
            commands::repo_credential_saved,
            commands::save_config,
            commands::list_repos,
            commands::add_local_repo,
            commands::remove_repo,
            commands::reorder_repos,
            commands::open_repo,
            commands::close_repo,
            commands::dest_status,
            commands::save_repo_settings,
            commands::new_repo,
            commands::refresh_status,
            commands::list_dir,
            commands::list_tree,
            commands::stage_paths,
            commands::unstage_paths,
            commands::discard_paths,
            commands::clean_worktree,
            commands::commit,
            commands::commit_preview,
            commands::last_commit_message,
            commands::diff_file,
            commands::commit_file_diff,
            commands::sync,
            commands::sync_preview,
            commands::sync_start,
            commands::cancel_job,
            commands::clone_repo,
            commands::save_credential,
            commands::history_page,
            commands::commit_detail,
            commands::list_branches,
            commands::branch_create,
            commands::branch_checkout,
            commands::list_tags,
            commands::checkout_target,
            commands::branch_rename,
            commands::branch_delete,
            commands::branch_merge,
            commands::merge_target,
            commands::get_effective_identity,
            commands::get_global_identity,
            commands::set_identity,
            commands::clear_repo_identity,
            commands::open_in_explorer,
            commands::open_config_dir,
            commands::open_url,
            commands::open_in_terminal,
            commands::edit_gitignore,
            commands::export_diagnostics,
        ])
        .run(tauri::generate_context!())
        .expect("error while running HiGit");
}
