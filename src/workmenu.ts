/** 工作区右键菜单：空白区 / 目录 / 文件三种形态（拉取/推送/提交/对比/撤销/分支操作/打开目录） */
import { store, showError } from "./store";
import { api } from "./api";
import { openCommitDialog, openSyncDialog } from "./tasks";
import { t } from "./i18n";
import type { CtxMenuItem } from "./ctxmenu";
import type { FileRowDto } from "./api/types";

/** 目标范围（目录 / 单文件）下可撤销修改的条目数；目录含其子路径，未跟踪目录聚合行也算 */
export function changedUnder(target: string): number {
  if (!store.status) return 0;
  return store.status.entries.filter(
    (e) => e.kind !== "ignored" && (!target || e.path === target || e.path.startsWith(`${target}/`)),
  ).length;
}

/** 打开撤销修改对话框：目录（isFile=false，空白区传当前浏览目录）或单个文件 */
export function openDiscard(target: string, isFile: boolean) {
  store.ui.discard = { target, isFile };
}

/** 打开清理对话框（git clean 语义，作用于整个工作区） */
export function openClean() {
  store.ui.clean = true;
}

/** 整个工作区是否存在可清理条目（未跟踪 / 被忽略）；无可清理时菜单项禁用 */
function hasCleanable(): boolean {
  return !!store.status?.entries.some((e) => e.kind === "untracked" || e.kind === "ignored");
}

/** 打开分支操作对话框：switch = 切换分支（git switch）；merge = 合并分支（git merge） */
export function openBranchOp(op: "switch" | "merge") {
  store.ui.branchOp = op;
}

/** 在系统文件管理器中打开当前面包屑导航到的目录（相对路径拼仓库根，根 = 仓库本身） */
function openCurrentDir() {
  // repoPath 是平台原生分隔符（Windows 反斜杠），统一成 / 后拼接，后端 to_path 会按平台还原
  const root = store.repoPath.replace(/\\/g, "/");
  const abs = store.currentDir ? `${root}/${store.currentDir}` : root;
  void api.openInExplorer(abs).catch(showError);
}

/** 菜单尾部公共项：打开所在文件夹 */
function explorerItems(): CtxMenuItem[] {
  return [
    { separator: true },
    { label: t("Open Containing Folder"), icon: "folder", action: openCurrentDir },
  ];
}

/** 空白区 / 背景菜单：撤销修改的目标为当前浏览目录 */
export function blankMenu(): CtxMenuItem[] {
  return [
    { label: t("Pull"), icon: "arrow-down", action: () => void openSyncDialog("pull") },
    { label: t("Push"), icon: "arrow-up", action: () => void openSyncDialog("push") },
    { separator: true },
    { label: t("Commit"), icon: "commit", action: () => openCommitDialog() },
    {
      label: t("Discard Changes"),
      icon: "restore",
      danger: true,
      disabled: changedUnder(store.currentDir) === 0,
      action: () => openDiscard(store.currentDir, false),
    },
    { label: t("Clean"), icon: "clean", disabled: !hasCleanable(), action: openClean },
    { separator: true },
    { label: t("Switch Branch"), icon: "branch", action: () => openBranchOp("switch") },
    { label: t("Merge Branch"), icon: "commit", action: () => openBranchOp("merge") },
    ...explorerItems(),
  ];
}

/** 文件 / 目录行菜单；对比变化仅文件行有，撤销修改单文件按状态可用 */
export function rowMenu(row: FileRowDto, openDiff: (row: FileRowDto) => void): CtxMenuItem[] {
  const items: CtxMenuItem[] = [
    { label: t("Pull"), icon: "arrow-down", action: () => void openSyncDialog("pull") },
    { label: t("Push"), icon: "arrow-up", action: () => void openSyncDialog("push") },
    { separator: true },
    { label: t("Commit"), icon: "commit", action: () => openCommitDialog() },
  ];
  const changeable = row.kind !== "clean" && row.kind !== "ignored";
  if (!row.isDir) {
    items.push({
      label: t("Compare Changes"),
      icon: "edit",
      disabled: !changeable,
      action: () => openDiff(row),
    });
  }
  items.push({
    label: t("Discard Changes"),
    icon: "restore",
    danger: true,
    disabled: row.isDir ? changedUnder(row.relPath) === 0 : !changeable,
    action: () => openDiscard(row.relPath, !row.isDir),
  });
  items.push(
    { label: t("Clean"), icon: "clean", disabled: !hasCleanable(), action: openClean },
    { separator: true },
    { label: t("Switch Branch"), icon: "branch", action: () => openBranchOp("switch") },
    { label: t("Merge Branch"), icon: "commit", action: () => openBranchOp("merge") },
    ...explorerItems(),
  );
  return items;
}
