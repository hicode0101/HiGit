/**
 * i18n：键 = 英文原文
 * 英文直接回退键本身，无需 en 资源。
 */
import { reactive } from "vue";
import { zhCN } from "./zh-CN";

export type Locale = "en" | "zh-CN";

const state = reactive<{ locale: Locale }>({ locale: "en" });

const DICTS: Record<Locale, Record<string, string>> = {
  en: {},
  "zh-CN": zhCN,
};

export function setLocale(l: Locale) {
  state.locale = l;
}

export function currentLocale(): Locale {
  return state.locale;
}

/** t("原文", { name: "x" }) → 本地化文案；{name} 占位符替换 */
export function t(key: string, params?: Record<string, string | number>): string {
  let text = DICTS[state.locale][key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.replaceAll(`{${k}}`, String(v));
    }
  }
  return text;
}

/** 相对时间本地化（FR-12.3）：zh "5 分钟前"，en "5 minutes ago" */
export function relativeTime(timeUnix: number): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000 - timeUnix));
  const units: Array<[string, string, number]> = [
    ["year", "years ago", 365 * 24 * 3600],
    ["month", "months ago", 30 * 24 * 3600],
    ["week", "weeks ago", 7 * 24 * 3600],
    ["day", "days ago", 24 * 3600],
    ["hour", "hours ago", 3600],
    ["minute", "minutes ago", 60],
  ];
  for (const [, key, secs] of units) {
    if (diff >= secs) {
      const n = Math.floor(diff / secs);
      if (state.locale === "zh-CN") {
        return `${n} ${t(key)}`;
      }
      return n === 1 ? `1 ${key.replace("s ago", " ago")}` : `${n} ${t(key)}`;
    }
  }
  return t("just now");
}

/** 本地化日期时间：2026-09-16 08:17 */
export function formatDateTime(timeUnix: number): string {
  const d = new Date(timeUnix * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 文件大小人性化显示：B / KB / MB / GB */
export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
