/** 共享输入校验 */

/** 邮箱格式：非空白@非空白.非空白（宽松校验，避免误拒合法地址） */
export const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** 邮箱格式是否合法（先 trim 再校验） */
export function isValidEmail(email: string): boolean {
  return EMAIL_RE.test(email.trim());
}

/** 仓库地址（远程 URL）：
 * - HTTPS / HTTP / ssh://：https://github.com/hicode0101/test1.git、ssh://git@github.com/hicode0101/HiGit.git
 * - SSH scp 风格：git@github.com:hicode0101/HiGit.git
 * 宽松校验：主机可为域名 / IP / localhost / SSH 别名，路径非空且不含空白 */
export const REPO_URL_RE = /^(?:https?|ssh):\/\/[^\s/]+\/[^\s]+$|^[^\s@]+@[^\s:]+:[^\s]+$/;

/** 仓库地址格式是否合法（先 trim 再校验） */
export function isValidRepoUrl(url: string): boolean {
  return REPO_URL_RE.test(url.trim());
}
