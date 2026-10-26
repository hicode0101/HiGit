<script setup lang="ts">
/** 提交对话框（FR-4 扩展，对齐 TortoiseGit Commit）：
 * 当前目录下全部可提交文件勾选列表（含 +/- 行数统计、状态快捷勾选、未版本控制开关、
 * 点击查看差异）+ 提交者 + 消息（字符计数 / Signed-off-by）+ Amend 修补 +
 * 提交到新分支 + 提交并推送 */
import { computed, onMounted, ref, watch } from "vue";
import { store, showError, toast } from "../store";
import { api } from "../api";
import type { CommitPreviewDto, StagedFileDto } from "../api/types";
import { doCommit, openSyncDialog } from "../tasks";
import { t, formatDateTime, formatSize } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

const preview = ref<CommitPreviewDto | null>(null);
const message = ref("");
const checked = ref<Set<string>>(new Set());
const committing = ref(false);
const enabling = ref(false);
/** 变更清单加载中（大仓库降级枚举可达数秒，避免空列表被误解为无变更） */
const loading = ref(true);

// ---- TortoiseGit 式选项 ----
const amend = ref(false);
const newBranchEnabled = ref(false);
const newBranchName = ref("");
const showUnversioned = ref(true);

const files = computed(() => preview.value?.files ?? []);
/** 未版本控制文件可整体隐藏 */
const visibleFiles = computed(() =>
  files.value.filter((f) => showUnversioned.value || f.kind !== "added"),
);
const totalCount = computed(() => visibleFiles.value.length);
const checkedCount = computed(() => visibleFiles.value.filter((f) => checked.value.has(f.path)).length);
const allChecked = computed(() => totalCount.value > 0 && checkedCount.value === totalCount.value);
const canCommit = computed(
  () =>
    !committing.value &&
    totalCount.value > 0 &&
    checkedCount.value > 0 &&
    message.value.trim().length > 0 &&
    (!newBranchEnabled.value || newBranchName.value.trim().length > 0),
);
const hasHead = computed(() => !!store.status?.head);
/** 因超过 Windows MAX_PATH(260) 被降级枚举隐藏的文件数 */
const longPathCount = computed(() => preview.value?.longPathCount ?? 0);
const longPathTip = computed(() => preview.value?.longPathSamples.join("\n") ?? "");

/** 提交范围 = 行右键选中的文件 / 目录（store.ui.commitScope），
 * 否则为变更页面包屑所在目录（根 = 整个仓库） */
const scopeDir = computed(() => store.ui.commitScope ?? store.currentDir);
const scopePrefix = computed(() => (scopeDir.value ? `${scopeDir.value}/` : ""));

/** 列表内展示相对当前目录的路径（悬停 title 显示完整路径） */
function displayPath(p: string): string {
  return scopePrefix.value && p.startsWith(scopePrefix.value)
    ? p.slice(scopePrefix.value.length)
    : p;
}

async function load() {
  loading.value = true;
  try {
    preview.value = await api.commitPreview(scopeDir.value);
    checked.value = new Set(preview.value.files.map((f) => f.path));
  } catch (e) {
    // 预览失败不再静默关窗：给出错误提示（此前超长路径问题因此长期黑盒）
    showError(e as Error);
    emit("close");
  } finally {
    loading.value = false;
  }
}

onMounted(load);

/** 启用仓库级长路径支持：写 core.longpaths=true → 重开会话（libgit2 按新配置
 * 枚举 / 暂存）→ 整表刷新，被隐藏的超长路径文件随之入列且可提交 */
async function enableLongPaths() {
  if (enabling.value) return;
  enabling.value = true;
  try {
    await api.enableCoreLongpaths();
    const { openRepo } = await import("../store");
    await openRepo(store.repoPath);
    toast(t("Long path support enabled"));
    await load();
  } catch (e) {
    showError(e as Error);
  } finally {
    enabling.value = false;
  }
}

function toggleAll(on: boolean) {
  checked.value = on ? new Set(visibleFiles.value.map((f) => f.path)) : new Set();
}

function toggleOne(path: string, on: boolean) {
  const next = new Set(checked.value);
  if (on) next.add(path);
  else next.delete(path);
  checked.value = next;
}

/** 快捷勾选（TortoiseGit Check: 行）：把符合条件的可见文件全部勾上 */
function checkWhere(pred: (f: StagedFileDto) => boolean) {
  checked.value = new Set(visibleFiles.value.filter(pred).map((f) => f.path));
}

// 隐藏未版本控制文件时，把已勾选的隐藏文件从提交范围剔除
watch(showUnversioned, (v) => {
  if (!v) {
    const keep = new Set(visibleFiles.value.map((f) => f.path));
    checked.value = new Set([...checked.value].filter((p) => keep.has(p)));
  }
});

// Amend 与新分支互斥；Amend 勾选时预填上一次提交消息
watch(amend, async (v) => {
  if (!v) return;
  newBranchEnabled.value = false;
  if (message.value.trim()) return;
  try {
    const last = await api.lastCommitMessage();
    if (last) message.value = last.body ? `${last.summary}\n\n${last.body}` : last.summary;
  } catch {
    /* 无历史提交时忽略 */
  }
});
watch(newBranchEnabled, (v) => {
  if (!v) return;
  amend.value = false;
  if (!newBranchName.value.trim()) {
    newBranchName.value = `${store.status?.branch ?? "main"}-new`;
  }
});

/** 在消息末尾追加 Signed-off-by（提交者身份） */
function addSignoff() {
  const who = preview.value?.committer;
  if (!who) return;
  const line = `Signed-off-by: ${who}`;
  if (message.value.includes(line)) return;
  const base = message.value.replace(/\s+$/, "");
  message.value = base ? `${base}\n\n${line}\n` : `${line}\n`;
}

async function submit(push = false) {
  if (!canCommit.value || committing.value) return;
  committing.value = true;
  // 勾选即提交：勾选文件先暂存（未暂存内容也纳入），未勾选但已暂存的先反暂存；
  // 再按勾选路径提交（git commit -- <paths>），范围外的暂存内容不受影响
  const checkedPaths = visibleFiles.value
    .filter((f) => checked.value.has(f.path))
    .map((f) => f.path);
  const unstagePaths = visibleFiles.value
    .filter((f) => !checked.value.has(f.path) && f.staged)
    .map((f) => f.path);
  const prep = unstagePaths.length ? { stage: checkedPaths, unstage: unstagePaths } : undefined;
  const nb = newBranchEnabled.value ? newBranchName.value.trim() : null;
  const ok = await doCommit(message.value.trim(), amend.value, prep, checkedPaths, nb);
  committing.value = false;
  if (ok) {
    emit("close");
    if (push) void openSyncDialog("push");
  }
}

function openDiff(f: StagedFileDto) {
  store.ui.diff = { path: f.path, spec: f.staged ? "staged" : "workdir" };
}

function statusLabel(kind: string): string {
  const map: Record<string, string> = {
    added: "New",
    modified: "Modified",
    deleted: "Deleted",
    renamed: "Renamed",
    conflicted: "Conflicted",
  };
  return t(map[kind] ?? kind);
}

function statusTip(kind: string): string {
  const map: Record<string, string> = {
    added: "New file (will be committed)",
    modified: "Modified file (will be committed)",
    deleted: "Deleted file (will be committed)",
    renamed: "Renamed file (will be committed)",
    conflicted: "Conflicted file (will be committed as resolved)",
  };
  return t(map[kind] ?? kind);
}

function statusIcon(kind: string): string {
  if (kind === "added") return "plus";
  if (kind === "deleted") return "minus";
  if (kind === "renamed") return "edit";
  return "warn";
}

function statusClass(kind: string): string {
  return kind === "renamed" ? "modified" : kind;
}
</script>

<template>
  <DialogBase :title="t('Commit')" :width="880" :close-on-mask="false" @close="emit('close')">
    <div class="repo-line">
      <span class="repo-name">{{ preview?.name ?? store.repoName }}</span>
      <span v-if="scopeDir" class="scope-dir">{{ scopeDir }}</span>
      <span
        v-if="preview?.committer"
        class="committer"
        :title="t('To change the committer, go to Repository Settings')"
      >
        <IconGlyph name="user" :size="14" />
        <span>{{ preview.committer }}</span>
      </span>
      <span v-else-if="preview" class="committer missing">{{ t("Identity not configured") }}</span>
    </div>

    <div class="opt-line">
      <label
        class="check-line"
        :class="{ off: newBranchEnabled || !hasHead }"
        :title="t('Replace the last commit with the checked changes (git commit --amend)')"
      >
        <input v-model="amend" type="checkbox" :disabled="newBranchEnabled || !hasHead" />
        <span>{{ t("Amend Last Commit") }}</span>
      </label>
      <label class="check-line" :class="{ off: amend }">
        <input v-model="newBranchEnabled" type="checkbox" :disabled="amend" />
        <span>{{ t("Commit to new branch") }}</span>
      </label>
      <input
        v-if="newBranchEnabled"
        v-model="newBranchName"
        class="nb-input"
        :placeholder="t('New branch name')"
      />
      <span class="fl-spacer"></span>
      <button
        class="btn mini-btn"
        :disabled="!preview?.committer"
        :title="t('Append Signed-off-by of the committer to the message')"
        @click="addSignoff"
      >
        {{ t("Add Signed-off-by") }}
      </button>
    </div>

    <textarea
      v-model="message"
      class="commit-msg"
      :placeholder="t('Commit message')"
      rows="4"
      :disabled="!!preview && totalCount === 0"
    />
    <div class="msg-count-line">
      <span class="msg-count">{{ message.length }}</span>
    </div>

    <div class="filter-line">
      <span class="fl-label">{{ t("Check") }}:</span>
      <button class="chip" @click="toggleAll(true)">{{ t("All") }}</button>
      <button class="chip" @click="toggleAll(false)">{{ t("None") }}</button>
      <button class="chip" @click="checkWhere((f) => f.kind === 'added')">
        {{ t("Unversioned") }}
      </button>
      <button class="chip" @click="checkWhere((f) => f.kind !== 'added')">
        {{ t("Versioned") }}
      </button>
      <button class="chip" @click="checkWhere((f) => f.kind === 'modified')">
        {{ t("Modified") }}
      </button>
      <button class="chip" @click="checkWhere((f) => f.kind === 'deleted')">
        {{ t("Deleted") }}
      </button>
      <span class="fl-spacer"></span>
      <label class="check-line" :title="t('Show or hide untracked files in the list')">
        <input v-model="showUnversioned" type="checkbox" />
        <span>{{ t("Show unversioned files") }}</span>
      </label>
    </div>

    <div v-if="longPathCount > 0" class="lp-hint">
      <IconGlyph name="warn" :size="14" />
      <span class="lp-text" :title="longPathTip">
        {{
          t("Files over the Windows 260-character path limit are hidden from the list", {
            n: longPathCount,
          })
        }}
      </span>
      <button
        class="chip"
        :disabled="enabling"
        :title="t('Write core.longpaths=true to the repository config, then reload')"
        @click="enableLongPaths"
      >
        {{ enabling ? t("Enabling…") : t("Enable long path support") }}
      </button>
    </div>

    <div class="file-list">
      <div class="list-head">
        <div class="col-check">
          <input
            type="checkbox"
            :checked="allChecked"
            :disabled="totalCount === 0"
            @change="toggleAll(($event.target as HTMLInputElement).checked)"
          />
        </div>
        <div class="col-path">{{ t("Path") }}</div>
        <div class="col-status">{{ t("Status") }}</div>
        <div class="col-add">{{ t("Lines Added") }}</div>
        <div class="col-del">{{ t("Lines Removed") }}</div>
        <div class="col-size">{{ t("Size") }}</div>
        <div class="col-mtime">{{ t("Last Modified") }}</div>
      </div>
      <div class="list-body">
        <div v-if="loading" class="list-loading">
          <div class="load-bar"></div>
          <span class="load-text">{{ t("Loading changed files…") }}</span>
        </div>
        <div v-for="f in visibleFiles" :key="f.path" class="file-row" @click="openDiff(f)">
          <div class="col-check" @click.stop>
            <input
              type="checkbox"
              :checked="checked.has(f.path)"
              @change="toggleOne(f.path, ($event.target as HTMLInputElement).checked)"
            />
          </div>
          <div class="col-path ellipsis" :title="f.path">{{ displayPath(f.path) }}</div>
          <div class="col-status">
            <span class="status-badge" :class="statusClass(f.kind)" :title="statusTip(f.kind)">
              <IconGlyph :name="statusIcon(f.kind)" :size="13" />
              <span>{{ statusLabel(f.kind) }}</span>
            </span>
          </div>
          <div class="col-add num" :class="{ none: !f.additions }">{{ f.additions ?? "—" }}</div>
          <div class="col-del num" :class="{ none: !f.deletions }">{{ f.deletions ?? "—" }}</div>
          <div class="col-size">{{ f.size == null ? "—" : formatSize(f.size) }}</div>
          <div class="col-mtime">{{ f.mtimeUnix ? formatDateTime(f.mtimeUnix) : "—" }}</div>
        </div>
        <div v-if="preview && totalCount === 0" class="empty-hint">{{ t("No changes") }}</div>
      </div>
    </div>

    <div class="dlg-actions">
      <span class="sel-count">
        <template v-if="loading">{{ t("Loading changed files…") }}</template>
        <template v-else-if="totalCount === 0">{{ t("No changes") }}</template>
        <template v-else>{{ t("Selected") }} {{ checkedCount }} / {{ totalCount }}</template>
      </span>
      <span class="push-btns">
        <button class="btn primary" :disabled="!canCommit" @click="submit(false)">
          {{ t("Commit") }}
        </button>
        <button
          class="btn"
          :disabled="!canCommit"
          :title="t('Commit, then open the push dialog')"
          @click="submit(true)"
        >
          {{ t("Commit & Push") }}
        </button>
      </span>
    </div>
  </DialogBase>
</template>

<style scoped>
.repo-line {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 8px;
  min-height: 22px;
}

.repo-name {
  font-size: 14px;
  font-weight: 600;
}

.scope-dir {
  font-size: 12px;
  color: var(--text-tertiary);
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
  padding: 2px 8px;
  cursor: default;
}

.committer {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: default;
}

.committer.missing {
  color: var(--status-modified);
}

.opt-line {
  display: flex;
  align-items: center;
  gap: 18px;
  margin-bottom: 8px;
}

.nb-input {
  width: 200px;
  font-size: 12px;
  padding: 3px 8px;
}

.mini-btn {
  font-size: 12px;
  padding: 3px 10px;
}

.commit-msg {
  width: 100%;
  resize: vertical;
  min-height: 80px;
  font-family: inherit;
}

.msg-count-line {
  display: flex;
  justify-content: flex-end;
  margin: -4px 0 6px;
}

.msg-count {
  font-size: 11px;
  color: var(--text-tertiary);
}

.filter-line {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
  flex-wrap: wrap;
}

.fl-label {
  font-size: 12px;
  color: var(--text-tertiary);
}

.chip {
  border: 1px solid var(--border-color);
  background: var(--bg-card);
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 10px;
  border-radius: 10px;
  cursor: pointer;
}

.chip:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.fl-spacer {
  flex: 1;
}

.file-list {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  overflow: hidden;
  max-height: 300px;
  display: flex;
  flex-direction: column;
}

.lp-hint {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
  padding: 6px 10px;
  border: 1px solid var(--status-modified);
  border-radius: var(--radius-md);
  background: var(--bg-hover);
  font-size: 12px;
  color: var(--text-secondary);
}

.lp-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: default;
}

.list-head {
  display: flex;
  align-items: center;
  padding: 7px 12px;
  border-bottom: 1px solid var(--divider);
  font-size: 12px;
  color: var(--text-tertiary);
  flex: none;
}

.list-body {
  overflow-y: auto;
}

.file-row {
  display: flex;
  align-items: center;
  padding: 6px 12px;
  border-bottom: 1px solid var(--divider);
  min-height: 34px;
  cursor: pointer;
}

.file-row:last-child {
  border-bottom: none;
}

.file-row:hover {
  background: var(--bg-hover);
}

.col-check {
  width: 36px;
  flex: none;
  display: flex;
  align-items: center;
}

.col-path {
  flex: 1;
  min-width: 0;
  font-size: 13px;
}

.col-status {
  width: 100px;
  flex: none;
}

.col-add,
.col-del {
  width: 64px;
  flex: none;
  font-size: 12px;
}

.col-add.num {
  color: var(--status-untracked, #3a9d5d);
}

.col-del.num {
  color: var(--status-deleted);
}

.col-add.num.none,
.col-del.num.none {
  color: var(--text-tertiary);
}

.col-size {
  width: 80px;
  flex: none;
  color: var(--text-secondary);
  font-size: 12px;
}

.col-mtime {
  width: 140px;
  flex: none;
  color: var(--text-tertiary);
  font-size: 12px;
}

.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  cursor: default;
}

.status-badge.added {
  color: var(--status-untracked);
}

.status-badge.modified {
  color: var(--status-modified);
}

.status-badge.deleted {
  color: var(--status-deleted);
}

.status-badge.conflicted {
  color: var(--status-conflict);
}

.empty-hint {
  text-align: center;
  color: var(--text-tertiary);
  padding: 28px 0;
}

.list-loading {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 30px 0 34px;
}

.load-bar {
  width: 220px;
  height: 4px;
  border-radius: 2px;
  background: var(--bg-hover);
  overflow: hidden;
  position: relative;
}

.load-bar::after {
  content: "";
  position: absolute;
  left: -40%;
  width: 40%;
  height: 100%;
  border-radius: 2px;
  background: var(--accent);
  animation: load-slide 1.1s ease-in-out infinite;
}

@keyframes load-slide {
  to {
    left: 100%;
  }
}

.load-text {
  font-size: 12px;
  color: var(--text-tertiary);
}

.dlg-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 12px;
}

.sel-count {
  font-size: 12px;
  color: var(--text-secondary);
}

.push-btns {
  display: flex;
  gap: 8px;
}

.check-line {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  cursor: pointer;
}

.check-line.off {
  color: var(--text-disabled);
}

.check-line input {
  accent-color: var(--accent);
  cursor: pointer;
}

.check-line input:disabled {
  cursor: default;
}
</style>
