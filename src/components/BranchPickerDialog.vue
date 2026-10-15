<script setup lang="ts">
/** 分支操作对话框（工作区右键）：switch = 切换分支（git switch）；
 * merge = 合并分支（git merge，显示当前分支并列出其它本地分支供选择） */
import { computed, ref, watch } from "vue";
import { store, showError, toast, refreshStatus, refreshRepos } from "../store";
import { api } from "../api";
import type { BranchDto } from "../api/types";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ close: [] }>();

const mode = computed(() => store.ui.branchOp ?? "switch");
const isSwitch = computed(() => mode.value === "switch");
const title = computed(() => (isSwitch.value ? t("Switch Branch") : t("Merge Branch")));

const branches = ref<BranchDto[]>([]);
const selected = ref("");
const busy = ref(false);

/** 切换：全部本地分支（当前分支置灰标记）；合并：排除当前分支 */
const candidates = computed(() => {
  const locals = branches.value.filter((b) => !b.isRemote);
  return isSwitch.value ? locals : locals.filter((b) => !b.isCurrent);
});

const currentBranch = computed(() => store.status?.branch ?? "");

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

function pick(b: BranchDto) {
  if (b.isCurrent || busy.value) return;
  selected.value = b.name;
}

async function confirm() {
  if (!selected.value || busy.value) return;
  busy.value = true;
  try {
    if (isSwitch.value) {
      await api.branchCheckout(selected.value);
      toast(`${t("Switched to")} ${selected.value}`);
    } else {
      const o = await api.branchMerge(selected.value);
      toast(o.updated ? (o.fastForward ? t("Fast-forwarded") : t("Merged")) : t("Already up to date"));
      if (o.conflicts.length > 0) {
        store.ui.error = {
          message: `${t("The following files have conflicts:")}\n${o.conflicts.join("\n")}`,
          raw: "",
        };
      }
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
  <DialogBase :title="title" :width="500" @close="emit('close')">
    <div class="hint-line">
      {{ isSwitch ? t("Select a branch to switch to") : t("Select a branch to merge into the current branch") }}
    </div>
    <div v-if="!isSwitch && currentBranch" class="cur-line">
      {{ t("Current Branch") }}: <b>{{ currentBranch }}</b>
    </div>
    <div class="branch-list">
      <div
        v-for="b in candidates"
        :key="b.name"
        class="branch-row"
        :class="{ current: b.isCurrent, selected: selected === b.name }"
        @click="pick(b)"
      >
        <IconGlyph name="branch" :size="14" />
        <span class="ellipsis b-name">{{ b.name }}</span>
        <span v-if="b.isCurrent" class="cur-badge">{{ t("current") }}</span>
        <span v-if="b.ahead || b.behind" class="ab">↑{{ b.ahead }} ↓{{ b.behind }}</span>
        <span class="b-summary ellipsis">{{ b.lastCommitSummary }}</span>
      </div>
      <div v-if="candidates.length === 0" class="empty">
        {{ isSwitch ? t("No local branches") : t("No other local branches") }}
      </div>
    </div>
    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!selected || busy" @click="confirm">
        {{ isSwitch ? t("Switch") : t("Merge") }}
      </button>
    </template>
  </DialogBase>
</template>

<style scoped>
.hint-line {
  color: var(--text-tertiary);
  font-size: 12px;
  margin-bottom: 8px;
}

.cur-line {
  font-size: 13px;
  color: var(--text-primary);
  margin-bottom: 8px;
}

.branch-list {
  max-height: 46vh;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--bg-card);
  padding: 4px;
}

.branch-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 36px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 13px;
}

.branch-row:hover {
  background: var(--bg-hover);
}

.branch-row.selected {
  background: var(--accent-subtle);
  box-shadow: inset 0 0 0 1px var(--accent);
}

.branch-row.current {
  color: var(--accent);
  font-weight: 600;
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

.empty {
  text-align: center;
  color: var(--text-tertiary);
  font-size: 12px;
  padding: 32px 0;
}
</style>
