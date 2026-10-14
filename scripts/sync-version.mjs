#!/usr/bin/env node
/**
 * 发布版本同步：把 tag 中的版本号写进三处版本源，保证一致后再编译打包——
 *   1. src-tauri/tauri.conf.json  version → 安装包文件名 / 元数据 / “关于”对话框
 *      （About 显示的 store.version 来自后端 app.package_info().version，即此字段）
 *   2. Cargo.toml  [workspace.package].version → exe 程序集属性
 *      （tauri-build 把 CARGO_PKG_VERSION 嵌入 Windows 版本资源，higit-app 继承 workspace 版本）
 *   3. package.json → 前端包版本（顺带保持一致）
 *
 * 用法：node scripts/sync-version.mjs <x.y.z>
 * 只接受纯数字三段式：WiX/MSI 的 ProductVersion 不支持预发布后缀（如 1.2.3-beta），
 * 带 - 后缀的 tag 会在这一步快速失败并给出明确提示。
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const version = process.argv[2]?.trim() ?? "";

if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(
    `[sync-version] 版本号必须是 x.y.z 纯数字三段式（MSI 不支持预发布后缀），收到: "${version}"`,
  );
  process.exit(1);
}

const changes = [];

// JSON 文件（package.json / tauri.conf.json）：解析 → 覆盖 version → 按 2 空格缩进写回。
// JSON.stringify 是确定性的：内容无变化时写回等价文本，无副作用。
function syncJson(relFile) {
  const file = join(root, relFile);
  const before = readFileSync(file, "utf8");
  const json = JSON.parse(before);
  const old = json.version;
  json.version = version;
  writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
  if (old !== version) changes.push(`${relFile}  ${old} -> ${version}`);
}

syncJson("package.json");
syncJson(join("src-tauri", "tauri.conf.json"));

// Cargo.toml：只改 [workspace.package] 段里的第一个 version（workspace 成员经
// version.workspace = true 继承），不碰 [workspace.dependencies] 里依赖的版本号
const cargoFile = join(root, "Cargo.toml");
const cargo = readFileSync(cargoFile, "utf8");
const header = cargo.indexOf("[workspace.package]");
if (header < 0) {
  console.error("[sync-version] Cargo.toml 中找不到 [workspace.package] 段");
  process.exit(1);
}
const rest = cargo.slice(header);
const matched = rest.match(/^version\s*=\s*"([^"]*)"/m);
if (!matched) {
  console.error("[sync-version] [workspace.package] 段中找不到 version 字段");
  process.exit(1);
}
if (matched[1] !== version) {
  writeFileSync(
    cargoFile,
    // /m 必须带上：rest 以 [workspace.package] 开头，无 /m 时 ^ 只匹配字符串开头，
    // 永远替换不到 version 行（会静默写回原文）
    cargo.slice(0, header) + rest.replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`),
  );
  changes.push(`Cargo.toml  ${matched[1]} -> ${version}`);
}

if (changes.length === 0) {
  console.log(`[sync-version] 三处版本号已全部是 ${version}，无需修改`);
} else {
  console.log(`[sync-version] 已同步版本号到 ${version}:`);
  for (const c of changes) console.log(`  - ${c}`);
}
// workspace 成员在 Cargo.lock 里的版本由 cargo 在构建时自动刷新，无需处理
console.log("[sync-version] done");
