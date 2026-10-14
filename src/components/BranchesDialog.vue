<script setup lang="ts">
/** 分支管理对话框（FR-7）：本地/远端分组、快速切换、新建、重命名、删除、合并 */
import { computed, ref, watch } from "vue";
import { store, showError, toast, refreshStatus, refreshRepos } from "../store";
import { api } from "../api";
import type { BranchDto } from "../api/types";
import { t } from "../i18n";
import { showConfirm, showInput } from "../store";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

const branches = ref<BranchDto[]>([]);
const newName = ref("");
const newFrom = ref("");
const checkoutNew = ref(true);
const busy = ref(false);

const locals = computed(() => branches.value.filter((b) => !b.isRemote));
const remotes = computed(() => branches.value.filter((b) => b.isRemote));
const hasHead = computed(() => !!store.status?.head);

async function load() {
  try {
    branches.value = await api.listBranches();
  } catch (e) {
    showError(e as Error);
  }
}

watch(
  () => store.repoPath,
  () => void load(),
);
void load();

async function run(fn: () => Promise<void>) {
  busy.value = true;
  try {
    await fn();
  } catch (e) {
    showError(e as Error);
  } finally {
    busy.value = false;
  }
}

async function switchTo(b: BranchDto) {
  if (b.isCurrent || b.isRemote || busy.value) return;
  await run(async () => {
    await api.branchCheckout(b.name);
    toast(`${t("Switched to")} ${b.name}`);
    await Promise.all([refreshStatus(), refreshRepos()]);
    await load();
  });
}

async function createBranch() {
  const name = newName.value.trim();
  if (!name) {
    store.ui.error = { message: t("Branch name is required"), raw: "" };
    return;
  }
  await run(async () => {
    await api.branchCreate(name, newFrom.value.trim(), checkoutNew.value);
    toast(t("Branch created"));
    newName.value = "";
    newFrom.value = "";
    await Promise.all([refreshStatus(), refreshRepos()]);
    await load();
  });
}

async function renameBranch(b: BranchDto) {
  const name = await showInput({ title: t("Rename branch"), label: t("Branch name"), value: b.name });
  if (!name || name === b.name) return;
  await run(async () => {
    await api.branchRename(b.name, name);
    toast(t("Branch renamed"));
    await load();
  });
}

async function deleteBranch(b: BranchDto) {
  const ok = await showConfirm({
    title: t("Branches"),
    message: `${t("Delete this branch?")} (${b.name})`,
    confirmLabel: t("Confirm"),
    danger: true,
  });
  if (!ok) return;
  try {
    await api.branchDelete(b.name, false);
    toast(t("Branch deleted"));
    await load();
  } catch (e) {
    // 未合并强确认（FR-7.3）
    if (e instanceof Error && "code" in e && (e as { code: string }).code === "branch-not-merged") {
      const force = await showConfirm({
        title: t("Branches"),
        message: t("This branch has unmerged commits. Delete it anyway?"),
        confirmLabel: t("Confirm"),
        danger: true,
      });
      if (force) {
        await run(async () => {
          await api.branchDelete(b.name, true);
          toast(t("Branch deleted"));
          await load();
        });
      }
    } else {
      showError(e as Error);
    }
  }
}

async function mergeBranch(b: BranchDto) {
  if (b.isRemote || busy.value) return;
  await run(async () => {
    const o = await api.branchMerge(b.name);
    toast(o.updated ? (o.fastForward ? t("Fast-forwarded") : t("Merged")) : t("Already up to date"));
    if (o.conflicts.length > 0) {
      store.ui.error = {
        message: `${t("The following files have conflicts:")}\n${o.conflicts.join("\n")}`,
        raw: "",
      };
    }
    await Promise.all([refreshStatus(), refreshRepos()]);
    await load();
  });
}
</script>

<template>
  <DialogBase :title="t('Branches')" :width="640" @close="emit('close')">
    <div v-if="hasHead" class="new-branch">
      <div class="nb-inputs">
        <input v-model="newName" :placeholder="t('Branch name')" />
        <input v-model="newFrom" class="from" :placeholder="t('Start point (empty = HEAD)')" />
        <label class="check-line">
          <input v-model="checkoutNew" type="checkbox" />
          <span>{{ t("Switch to the new branch") }}</span>
        </label>
      </div>
      <button class="btn primary" :disabled="busy || !newName.trim()" @click="createBranch">
        {{ t("Create") }}
      </button>
    </div>

    <div class="branch-groups">
      <div class="group-title">{{ t("Local") }}</div>
      <div v-for="b in locals" :key="b.name" class="branch-row" :class="{ current: b.isCurrent }" @click="switchTo(b)">
        <IconGlyph name="branch" :size="14" />
        <span class="ellipsis b-name">{{ b.name }}</span>
        <span v-if="b.isCurrent" class="cur-badge">{{ t("current") }}</span>
        <span v-if="b.ahead || b.behind" class="ab">↑{{ b.ahead }} ↓{{ b.behind }}</span>
        <span class="b-summary ellipsis">{{ b.lastCommitSummary }}</span>
        <span class="row-actions">
          <button class="mini" :title="t('Rename branch')" @click.stop="renameBranch(b)"><IconGlyph name="edit" :size="13" /></button>
          <button class="mini" :title="t('Merge')" @click.stop="mergeBranch(b)"><IconGlyph name="commit" :size="13" /></button>
          <button class="mini danger" :title="t('Branch deleted')" @click.stop="deleteBranch(b)"><IconGlyph name="trash" :size="13" /></button>
        </span>
      </div>
      <div v-if="locals.length === 0" class="empty">{{ t("No local branches") }}</div>

      <div class="group-title">{{ t("Remote") }}</div>
      <div v-for="b in remotes" :key="b.name" class="branch-row remote">
        <IconGlyph name="branch" :size="14" />
        <span class="ellipsis b-name">{{ b.name }}</span>
        <span class="b-summary ellipsis">{{ b.lastCommitSummary }}</span>
      </div>
    </div>
  </DialogBase>
</template>

<style scoped>
.new-branch {
  display: flex;
  gap: 10px;
  align-items: flex-end;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--divider);
  margin-bottom: 10px;
}

.nb-inputs {
  flex: 1;
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}

/* 只作用于文本输入框；复选框会 otherwise 被 flex:1 拉长变形 */
.nb-inputs input:not([type="checkbox"]) {
  flex: 1;
  min-width: 120px;
}

.nb-inputs .from {
  flex: 1.2;
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

.branch-groups {
  max-height: 50vh;
  overflow-y: auto;
}

.group-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.4px;
  padding: 10px 4px 6px;
}

.branch-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 8px;
  /* 固定最低行高：hover 浮现的操作按钮不再把行撑高，避免界面晃动 */
  min-height: 38px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 13px;
}

.branch-row:hover {
  background: var(--bg-hover);
}

.branch-row.current {
  color: var(--accent);
  font-weight: 600;
}

.branch-row.remote {
  color: var(--text-secondary);
  cursor: default;
}

.b-name {
  max-width: 220px;
}

.cur-badge {
  flex: none;
  font-size: 10px;
  background: var(--accent);
  color: #fff;
  border-radius: 8px;
  padding: 1px 8px;
}

.ab {
  flex: none;
  font-size: 11px;
  color: var(--status-modified);
}

.b-summary {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  color: var(--text-tertiary);
  font-weight: 400;
}

.row-actions {
  display: none;
  gap: 2px;
  flex: none;
}

.branch-row:hover .row-actions {
  display: inline-flex;
}

.mini {
  border: none;
  background: none;
  color: var(--text-tertiary);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
}

.mini:hover {
  background: var(--bg-active);
  color: var(--text-primary);
}

.mini.danger:hover {
  color: var(--status-deleted);
}

.empty {
  color: var(--text-tertiary);
  font-size: 12px;
  padding: 6px;
}
</style>
