//! .gitignore 模板（FR-9.2）：按语言栈生成；新建仓库对话框选择。
//!
//! 模板内容为常用忽略项的最小集合（参考 github/gitignore 精简），
//! 内嵌编译进二进制，无需运行时下载。

/// 语言栈选项（ComboBox 展示顺序）；首项为通用模板。
pub const STACKS: &[&str] = &[
    "General", "Rust", "Node.js", "Python", "Go", "Java", "C++", "C#",
];

/// 返回指定语言栈的 .gitignore 内容；未知栈回退到 General。
pub fn template(stack: &str) -> &'static str {
    match stack {
        "Rust" => RUST,
        "Node.js" => NODE,
        "Python" => PYTHON,
        "Go" => GO,
        "Java" => JAVA,
        "C++" => CPP,
        "C#" => CSHARP,
        _ => GENERAL,
    }
}

const GENERAL: &str = "\
# Build artifacts
/target/
/build/
/bin/
/out/
*.log

# OS
.DS_Store
Thumbs.db
";

const RUST: &str = "\
/target/
debug/
release/

# Cargo lock is versioned for applications; uncomment for libraries
# Cargo.lock

*.log
.DS_Store
Thumbs.db
";

const NODE: &str = "\
node_modules/
dist/
build/
coverage/
.npm/
.eslintcache

.env
.env.*
npm-debug.log*
yarn-debug.log*
yarn-error.log*

.DS_Store
Thumbs.db
";

const PYTHON: &str = "\
__pycache__/
*.py[cod]
*.egg-info/
.eggs/
build/
dist/

.venv/
venv/
env/

.pytest_cache/
.mypy_cache/
.ruff_cache/
.coverage
htmlcov/

.env
.DS_Store
Thumbs.db
";

const GO: &str = "\
/bin/
/pkg/
*.exe
*.test
*.out
vendor/

.env
.DS_Store
Thumbs.db
";

const JAVA: &str = "\
target/
build/
out/
*.class
*.jar
*.war

.gradle/
.idea/
*.iml
.classpath
.project
.settings/

.DS_Store
Thumbs.db
";

const CPP: &str = "\
build/
bin/
obj/
out/
cmake-build-*/
CMakeLists.txt.user
*.o
*.obj
*.so
*.dylib
*.dll
*.exe

compile_commands.json
.DS_Store
Thumbs.db
";

const CSHARP: &str = "\
bin/
obj/
out/
*.user
*.suo
.vs/
.idea/
packages/
TestResults/

.DS_Store
Thumbs.db
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_stacks_have_templates() {
        assert!(!STACKS.is_empty());
        for stack in STACKS {
            let body = template(stack);
            assert!(body.len() > 10, "template for {stack} must not be empty");
        }
    }

    #[test]
    fn rust_template_ignores_target() {
        assert!(template("Rust").contains("/target/"));
    }

    #[test]
    fn unknown_stack_falls_back_to_general() {
        assert_eq!(template("Klingon"), template("General"));
    }
}
