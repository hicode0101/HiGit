//! HiGit 的 Git 引擎抽象层。
//!
//! 上层（higit-core / higit-ui）只允许依赖 [`engine`]、[`model`]、[`error`]、[`factory`]
//! 中的类型；具体后端（git2 / 未来可能的 gix、CLI）的类型不得泄漏出去，
//! 这是保证后端可替换的硬约束（docs/technical-design.md CR-1）。

pub mod engine;
pub mod error;
pub mod factory;
pub mod model;

#[cfg(feature = "backend-git2")]
pub(crate) mod backend_git2;
