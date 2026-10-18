<script setup lang="ts">
/** 合并分支对话框（工作区右键，对齐 TortoiseGit Merge）：
 * 来源支持 分支（本地 + 远程）/ 标签 / 任意提交；
 * 选项：--no-ff / --ff-only / --no-commit / --squash、附加日志消息（merge.log）、
 * 自定义合并提交消息、-s ours 策略（libgit2 仅支持默认与 ours 两种策略） */
import { computed, ref, watch } from "vue";
import { store, showError, toast, refreshStatus, refreshRepos } from "../store";
import { api } from "../api";
import type { BranchDto, CommitEntryDto, MergeOptsDto, TagDto } from "../api/types";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

type SourceTab = "branch" | "tag" | "commit";
const tab = ref<SourceTab>("branch");
const filter = ref("");

const branches = ref<BranchDto[]>([]);
const tags = ref<TagDto[]>([]);
const commits = ref<CommitEntryDto[]>([]);

const pickedBranch = ref("");
const pickedTag = ref("");
const commitHash = ref("");
const busy = ref(false);

// ---- 选项区（对应 TortoiseGit Option） ----
const noFF = ref(false);
const ffOnly = ref(false);
const noCommit = ref(false);
const squash = ref(false);
const withLog = ref(false);
const logCount = ref(20);
const strategy = ref("");
const message = ref("");

const hasHead = computed(() => !!store.status?.head);
const currentBranch = computed(() => store.status?.branch ?? "");

const localBranches = computed(() =>
  branches.value.filter(
    (b) => !b.isRemote && !b.isCurrent && b.name.includes(filter.value.trim()),
  ),
);
const remoteBranches = computed(() =>
  branches.value.filter((b) => b.isRemote && b.name.includes(filter.value.trim())),
);
const tagList = computed(() => tags.value.filter((x) => x.name.includes(filter.value.trim())));

/** 当前选中来源（跟随页签） */
const target = computed(() => {
  if (tab.value === "branch") return pickedBranch.value;
  if (tab.value === "tag") return pickedTag.value;
  return commitHash.value.trim();
});

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

function pickBranch(b: BranchDto) {
  if (busy.value) return;
  pickedBranch.value = b.name;
}

function pickTag(x: TagDto) {
  if (busy.value) return;
  pickedTag.value = x.name;
}

function pickCommit(c: CommitEntryDto) {
  if (busy.value) return;
  commitHash.value = c.id;
}

watch(tab, () => {
  filter.value = "";
});

// --no-ff 与 --ff-only 互斥；--squash 隐含 --no-commit
watch(noFF, (v) => {
  if (v) ffOnly.value = false;
});
watch(ffOnly, (v) => {
  if (v) noFF.value = false;
});

async function confirm() {
  const tgt = target.value;
  if (!tgt || busy.value) return;
  busy.value = true;
  try {
    const opts: MergeOptsDto = {
      kind: tab.value,
      source: tgt,
      noFf: noFF.value,
      ffOnly: ffOnly.value,
      noCommit: !squash.value && noCommit.value,
      squash: squash.value,
      message: message.value.trim() || null,
      logMessages: withLog.value ? Math.max(1, Math.min(100, logCount.value || 20)) : 0,
      strategy: strategy.value,
    };
    const o = await api.mergeTarget(opts);
    if (o.conflicts.length > 0) {
      store.ui.error = {
        message: `${t("The following files have conflicts:")}\n${o.conflicts.join("\n")}`,
        raw: "",
      };
    } else if (o.pendingCommit) {
      toast(
        squash.value
          ? t("Squash merge completed; changes are staged, commit when ready")
          : t("Merge completed without commit; review and commit manually"),
      );
    } else if (o.updated) {
      toast(o.fastForward ? t("Fast-forwarded") : t("Merged"));
    } else {
      toast(t("Already up to date"));
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
  <DialogBase :title="t('Merge Branch')" :width="560" @close="emit('close')">
    <div class="section-title">{{ t("Merge From") }}</div>
    <div v-if="currentBranch" class="cur-line">
      {{ t("Current Branch") }}: <b>{{ currentBranch }}</b>
    </div>
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
            :class="{ selected: pickedBranch === b.name }"
            @click="pickBranch(b)"
          >
            <IconGlyph name="branch" :size="14" />
            <span class="ellipsis p-name">{{ b.name }}</span>
            <span v-if="b.ahead || b.behind" class="ab">↑{{ b.ahead }} ↓{{ b.behind }}</span>
            <span class="p-summary ellipsis">{{ b.lastCommitSummary }}</span>
          </div>
          <div v-if="localBranches.length === 0" class="empty">{{ t("No other local branches") }}</div>

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

    <!-- 提交来源：手输 hash / 表达式 + 最近提交点选 -->
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
      <label
        class="check-line"
        :title="t('Stage the merge result without creating a merge commit (git merge --squash)')"
      >
        <input v-model="squash" type="checkbox" />
        <span>{{ t("Squash") }}</span>
      </label>
      <span class="log-cell">
        <label
          class="check-line"
          :title="t('Append recent commit summaries of the merged branch to the message (merge.log)')"
        >
          <input v-model="withLog" type="checkbox" />
          <span>{{ t("Messages") }}</span>
        </label>
        <input
          v-model.number="logCount"
          type="number"
          class="log-count"
          min="1"
          max="100"
          :disabled="!withLog"
        />
      </span>

      <label
        class="check-line"
        :title="t('Always create a merge commit even when fast-forward is possible (git merge --no-ff)')"
      >
        <input v-model="noFF" type="checkbox" />
        <span>{{ t("No Fast Forward") }}</span>
      </label>
      <label
        class="check-line"
        :title="t('Abort unless the merge can be resolved as a fast-forward (git merge --ff-only)')"
      >
        <input v-model="ffOnly" type="checkbox" />
        <span>{{ t("Fast Forward Only") }}</span>
      </label>

      <label
        class="check-line"
        :class="{ off: squash }"
        :title="t('Perform the merge but stop before creating the merge commit (git merge --no-commit)')"
      >
        <input v-model="noCommit" type="checkbox" :disabled="squash" />
        <span>{{ t("No Commit") }}</span>
      </label>
      <label class="check-line strategy-line" :title="t('libgit2 supports the default strategy and ours only')">
        <span>{{ t("Strategy") }}</span>
        <select v-model="strategy" class="strategy-select">
          <option value="">{{ t("Default") }}</option>
          <option value="ours">ours (-s ours)</option>
        </select>
      </label>
    </div>

    <div class="section-title">{{ t("Merge Message") }}</div>
    <textarea
      v-model="message"
      class="merge-msg"
      rows="4"
      :placeholder="t('<Auto Generated by Git>')"
    ></textarea>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!target || busy" @click="confirm">
        {{ t("Merge") }}
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

.cur-line {
  font-size: 12px;
  color: var(--text-secondary);
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
  max-height: 28vh;
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

.log-cell {
  display: flex;
  align-items: center;
  gap: 8px;
}

.log-count {
  width: 64px;
  padding: 2px 6px;
  font-size: 12px;
}

.strategy-line {
  gap: 8px;
}

.strategy-select {
  flex: 1;
  min-width: 0;
  font-size: 12px;
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

.merge-msg {
  width: 100%;
  resize: vertical;
  font-size: 12px;
  font-family: Consolas, monospace;
}

.empty {
  text-align: center;
  color: var(--text-tertiary);
  font-size: 12px;
  padding: 24px 0;
}
</style>
