<script setup lang="ts">
/** 切换分支对话框（工作区右键「切换分支」，对齐 TortoiseGit Switch/Checkout）：
 * 目标支持 分支（本地 + 远程）/ 标签 / 任意提交；
 * 选项：新建分支、强制检出（覆盖工作区修改）、合并本地修改（git checkout -m 语义）、
 * 跟踪远程分支（git checkout --track）、覆盖已存在分支（git branch -f） */
import { computed, ref, watch } from "vue";
import { store, showError, toast, refreshStatus, refreshRepos } from "../store";
import { api } from "../api";
import type { BranchDto, CheckoutOptsDto, CommitEntryDto, TagDto } from "../api/types";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

type TargetTab = "branch" | "tag" | "commit";
const tab = ref<TargetTab>("branch");
const filter = ref("");

const branches = ref<BranchDto[]>([]);
const tags = ref<TagDto[]>([]);
const commits = ref<CommitEntryDto[]>([]);

const pickedBranch = ref("");
const pickedTag = ref("");
const commitHash = ref("");
const busy = ref(false);

// ---- 选项区（对应 TortoiseGit Option） ----
const createNew = ref(false);
const newBranchName = ref("");
/** 用户手动编辑过新分支名后不再自动预填 */
const newBranchDirty = ref(false);
const force = ref(false);
const merge = ref(false);
const track = ref(true);
const overrideExisting = ref(false);

const hasHead = computed(() => !!store.status?.head);

const localBranches = computed(() =>
  branches.value.filter((b) => !b.isRemote && b.name.includes(filter.value.trim())),
);
const remoteBranches = computed(() =>
  branches.value.filter((b) => b.isRemote && b.name.includes(filter.value.trim())),
);
const tagList = computed(() => tags.value.filter((x) => x.name.includes(filter.value.trim())));

/** 当前选中目标（跟随页签） */
const target = computed(() => {
  if (tab.value === "branch") return pickedBranch.value;
  if (tab.value === "tag") return pickedTag.value;
  return commitHash.value.trim();
});

const pickedRemote = computed(() =>
  branches.value.find((b) => b.isRemote && b.name === pickedBranch.value),
);
/** 远端分支对应的本地分支名（origin/p2-1 → p2-1） */
const autoTrackName = computed(() =>
  pickedRemote.value ? pickedRemote.value.name.replace(/^[^/]+\//, "") : "",
);
/** 本次操作是否会产生新本地分支（决定「覆盖已存在的分支」是否可用） */
const branchWillCreate = computed(() => createNew.value || (!!pickedRemote.value && track.value));

async function load() {
  try {
    const jobs: Promise<void>[] = [
      api
        .listBranches()
        .then((r) => {
          branches.value = r;
        })
        .catch((e) => showError(e as Error)),
      api
        .listTags()
        .then((r) => {
          tags.value = r;
        })
        .catch(() => {
          tags.value = [];
        }),
    ];
    if (hasHead.value) {
      jobs.push(
        api
          .historyPage(30, 0)
          .then((r) => {
            commits.value = r;
          })
          .catch(() => {
            commits.value = [];
          }),
      );
    }
    await Promise.all(jobs);
  } catch (e) {
    showError(e as Error);
  }
}
watch(
  () => store.repoPath,
  () => void load(),
);
void load();

/** 新建分支名的自动预填：选中的分支 / 标签名（远端去掉远端前缀）；提交为 Branch_<短hash> */
function suggestName() {
  if (newBranchDirty.value) return;
  if (tab.value === "tag") newBranchName.value = pickedTag.value;
  else if (tab.value === "branch") newBranchName.value = autoTrackName.value || pickedBranch.value;
  else {
    const c = commits.value.find((x) => x.id === commitHash.value.trim());
    newBranchName.value = c ? `Branch_${c.shortId}` : "";
  }
}

function pickBranch(b: BranchDto) {
  if (busy.value) return;
  pickedBranch.value = b.name;
  // 与 TortoiseGit 一致：选中远端分支默认建立跟踪
  track.value = b.isRemote;
  suggestName();
}

function pickTag(x: TagDto) {
  if (busy.value) return;
  pickedTag.value = x.name;
  suggestName();
}

function pickCommit(c: CommitEntryDto) {
  if (busy.value) return;
  commitHash.value = c.id;
  suggestName();
}

watch(tab, () => {
  filter.value = "";
  track.value = tab.value === "branch" && !!pickedRemote.value;
  suggestName();
});
watch(createNew, (v) => {
  if (v && !newBranchName.value.trim()) {
    newBranchDirty.value = false;
    suggestName();
  }
});
// 强制（丢弃修改）与合并（携带修改）互斥
watch(force, (v) => {
  if (v) merge.value = false;
});
watch(merge, (v) => {
  if (v) force.value = false;
});

async function confirm() {
  const tgt = target.value;
  if (!tgt || busy.value) return;
  if (createNew.value && !newBranchName.value.trim()) {
    store.ui.error = { message: t("Branch name is required"), raw: "" };
    return;
  }
  busy.value = true;
  try {
    const opts: CheckoutOptsDto = {
      kind: tab.value,
      target: tgt,
      createBranch: createNew.value ? newBranchName.value.trim() : null,
      force: force.value,
      merge: merge.value,
      track: track.value,
      overrideExisting: overrideExisting.value,
    };
    const o = await api.checkoutTarget(opts);
    if (o.conflicted) {
      toast(t("Switched with conflicts; resolve them in Changes"));
    } else if (o.createdBranch) {
      toast(`${t("Switched to")} ${o.createdBranch}`);
    } else if (o.detached) {
      toast(`${t("Checked out")} ${tgt} (${t("detached HEAD")})`);
    } else {
      toast(`${t("Switched to")} ${tgt}`);
    }
    await Promise.all([refreshStatus(), refreshRepos()]);
    emit("close");
  } catch (e) {
    showError(e as Error);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <DialogBase :title="t('Switch Branch')" :width="560" @close="emit('close')">
    <div class="section-title">{{ t("Switch To") }}</div>
    <div class="seg-row">
      <label class="seg">
        <input v-model="tab" type="radio" value="branch" />
        <span>{{ t("Branch") }}</span>
      </label>
      <label class="seg">
        <input v-model="tab" type="radio" value="tag" />
        <span>{{ t("Tag") }}</span>
      </label>
      <label class="seg">
        <input v-model="tab" type="radio" value="commit" />
        <span>{{ t("Commit") }}</span>
      </label>
    </div>

    <!-- 分支 / 标签选择列表 -->
    <template v-if="tab !== 'commit'">
      <input v-model="filter" class="filter" :placeholder="t('Filter')" />
      <div class="pick-list">
        <template v-if="tab === 'branch'">
          <div class="group-title">{{ t("Local") }}</div>
          <div
            v-for="b in localBranches"
            :key="b.name"
            class="pick-row"
            :class="{ current: b.isCurrent, selected: pickedBranch === b.name }"
            @click="pickBranch(b)"
          >
            <IconGlyph name="branch" :size="14" />
            <span class="ellipsis p-name">{{ b.name }}</span>
            <span v-if="b.isCurrent" class="cur-badge">{{ t("current") }}</span>
            <span v-if="b.ahead || b.behind" class="ab">↑{{ b.ahead }} ↓{{ b.behind }}</span>
            <span class="p-summary ellipsis">{{ b.lastCommitSummary }}</span>
          </div>
          <div v-if="localBranches.length === 0" class="empty">{{ t("No local branches") }}</div>

          <div class="group-title">{{ t("Remote") }}</div>
          <div
            v-for="b in remoteBranches"
            :key="b.name"
            class="pick-row"
            :class="{ selected: pickedBranch === b.name }"
            @click="pickBranch(b)"
          >
            <IconGlyph name="branch" :size="14" />
            <span class="ellipsis p-name">{{ b.name }}</span>
            <span class="p-id">{{ b.shortId }}</span>
            <span class="p-summary ellipsis">{{ b.lastCommitSummary }}</span>
          </div>
          <div v-if="remoteBranches.length === 0" class="empty">{{ t("No remote branches") }}</div>
        </template>

        <template v-else>
          <div
            v-for="x in tagList"
            :key="x.name"
            class="pick-row"
            :class="{ selected: pickedTag === x.name }"
            @click="pickTag(x)"
          >
            <IconGlyph name="tag" :size="14" />
            <span class="ellipsis p-name">{{ x.name }}</span>
            <span class="p-id">{{ x.shortId }}</span>
            <span class="p-summary ellipsis">{{ x.lastCommitSummary }}</span>
          </div>
          <div v-if="tagList.length === 0" class="empty">{{ t("No tags") }}</div>
        </template>
      </div>
    </template>

    <!-- 提交目标：手输 hash / 表达式 + 最近提交点选 -->
    <template v-else>
      <input
        v-model="commitHash"
        class="filter"
        :placeholder="t('Commit hash or expression (e.g. HEAD~1)')"
      />
      <div class="pick-list">
        <div class="group-title">{{ t("Recent Commits") }}</div>
        <div
          v-for="c in commits"
          :key="c.id"
          class="pick-row"
          :class="{ selected: commitHash.trim() === c.id }"
          @click="pickCommit(c)"
        >
          <IconGlyph name="commit" :size="14" />
          <span class="p-id">{{ c.shortId }}</span>
          <span class="p-summary ellipsis">{{ c.summary }}</span>
        </div>
        <div v-if="commits.length === 0" class="empty">{{ t("No commits") }}</div>
      </div>
    </template>

    <div class="section-title">{{ t("Options") }}</div>
    <div class="opt-grid">
      <label class="check-line">
        <input v-model="createNew" type="checkbox" />
        <span>{{ t("Create New Branch") }}</span>
      </label>
      <input
        v-model="newBranchName"
        class="nb-name"
        :disabled="!createNew"
        :placeholder="t('Branch name')"
        @input="newBranchDirty = true"
      />

      <label
        class="check-line"
        :title="t('Discard local changes that would be overwritten (git checkout -f)')"
      >
        <input v-model="force" type="checkbox" />
        <span>{{ t("Overwrite working tree changes (force)") }}</span>
      </label>
      <label
        class="check-line"
        :title="t('Carry uncommitted changes to the new branch (like git checkout -m)')"
      >
        <input v-model="merge" type="checkbox" />
        <span>{{ t("Merge local changes") }}</span>
      </label>

      <label
        class="check-line"
        :class="{ off: tab !== 'branch' || !pickedRemote }"
        :title="t('Create a local branch with the same name tracking it (git checkout --track)')"
      >
        <input v-model="track" type="checkbox" :disabled="tab !== 'branch' || !pickedRemote" />
        <span>{{ t("Track remote branch") }}</span>
      </label>
      <label
        class="check-line"
        :class="{ off: !branchWillCreate }"
        :title="t('Force-move the branch to the target if it already exists (git branch -f)')"
      >
        <input v-model="overrideExisting" type="checkbox" :disabled="!branchWillCreate" />
        <span>{{ t("Override branch if exists") }}</span>
      </label>
    </div>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!target || busy" @click="confirm">
        {{ t("Switch") }}
      </button>
    </template>
  </DialogBase>
</template>

<style scoped>
.section-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.4px;
  margin-bottom: 8px;
}

.seg-row {
  display: flex;
  gap: 16px;
  margin-bottom: 10px;
}

.seg {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text-primary);
  cursor: pointer;
}

.seg input {
  accent-color: var(--accent);
  cursor: pointer;
}

.filter {
  margin-bottom: 8px;
}

.pick-list {
  max-height: 32vh;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--bg-card);
  padding: 4px;
  margin-bottom: 12px;
}

.group-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.4px;
  padding: 8px 4px 4px;
}

.pick-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 34px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 13px;
  color: var(--text-primary);
}

.pick-row:hover {
  background: var(--bg-hover);
}

.pick-row.selected {
  background: var(--accent-subtle);
  box-shadow: inset 0 0 0 1px var(--accent);
}

.pick-row.current {
  color: var(--accent);
  font-weight: 600;
}

.p-name {
  max-width: 200px;
}

.p-id {
  flex: none;
  font-size: 11px;
  color: var(--text-tertiary);
  font-family: Consolas, monospace;
}

.p-summary {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  color: var(--text-tertiary);
  font-weight: 400;
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

.opt-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px 16px;
  align-items: center;
}

.nb-name {
  min-width: 0;
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

.empty {
  text-align: center;
  color: var(--text-tertiary);
  font-size: 12px;
  padding: 24px 0;
}
</style>
