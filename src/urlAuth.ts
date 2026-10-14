/** 根据远端 URL 推断认证方式与用户名（对话框里修改 URL 时自动切换认证页）。
 *
 * 规则：
 * - `https://github.com/用户名/仓库名.git` → HTTPS，用户名取 URL 里的「用户名」段
 * - `git@github.com:用户名/仓库名.git`（SCP 风格）或 `ssh://git@host/...` → SSH，
 *   用户名取 @ 前的值（缺省 git）
 * - 其余（本地路径、无法识别）返回 null，不改动当前认证设置
 */
export function detectAuthFromUrl(raw: string): { kind: "http" | "ssh"; username: string } | null {
  const url = raw.trim();
  if (!url) return null;
  if (/^https?:\/\//i.test(url)) {
    // owner = 主机后第一段路径；宽松解析，取不到就不预填
    let owner = "";
    try {
      owner = new URL(url).pathname.split("/").find(Boolean) ?? "";
    } catch {
      owner = "";
    }
    return { kind: "http", username: owner };
  }
  // git@host:path 与 ssh://user@host[:port]/path（@ 前的值即登录用户名）
  const m = url.match(/^(?:ssh:\/\/)?([^@/\s]+)@[\w.-]+[:/]/i);
  if (m) return { kind: "ssh", username: m[1] || "git" };
  return null;
}
