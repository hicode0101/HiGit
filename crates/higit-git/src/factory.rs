//! 后端工厂：应用启动时按配置注入实现，未来切换后端只改这里。

use crate::engine::GitEngine;
use crate::error::{HigitError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// git2-rs（静态链接 libgit2），当前唯一可用后端
    Git2,
    /// 预留：gitoxide（待 gix 补齐 push）
    Gix,
    /// 预留：调用系统 git CLI 兜底
    Cli,
}

pub fn create_backend(kind: BackendKind) -> Result<Box<dyn GitEngine>> {
    match kind {
        #[cfg(feature = "backend-git2")]
        BackendKind::Git2 => Ok(Box::new(crate::backend_git2::Git2Backend::new())),
        #[cfg(not(feature = "backend-git2"))]
        BackendKind::Git2 => Err(HigitError::not_implemented("backend-git2 feature disabled")),

        BackendKind::Gix => Err(HigitError::not_implemented("backend-gix")),
        BackendKind::Cli => Err(HigitError::not_implemented("backend-cli")),
    }
}
