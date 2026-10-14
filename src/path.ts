/** 平台路径工具：目录分隔符按平台区分（Windows 反斜杠，macOS/Linux 正斜杠） */

export const nativeSep = /^win/i.test(navigator.platform) ? "\\" : "/";

/** 拼接父目录与子路径名，尾部分隔符归一 */
export function joinNative(dir: string, name: string) {
  return dir.replace(/[\\/]+$/, "") + nativeSep + name;
}
