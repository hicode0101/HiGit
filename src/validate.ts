/** 共享输入校验 */

/** 邮箱格式：非空白@非空白.非空白（宽松校验，避免误拒合法地址） */
export const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** 邮箱格式是否合法（先 trim 再校验） */
export function isValidEmail(email: string): boolean {
  return EMAIL_RE.test(email.trim());
}
