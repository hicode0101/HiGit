<script setup lang="ts">
/** 状态栏（图1）：分支 / 最近提交 / 当前用户 / 任务状态 / 同步状态 / 图例 */
import { computed } from "vue";
import { store } from "../store";
import { t, relativeTime, formatDateTime } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ openBranches: []; syncClick: [kind: "push" | "pull"]; openLegend: [] }>();

const branch = computed(() => store.status?.branch ?? "");
const head = computed(() => store.status?.head ?? null);
const ahead = computed(() => store.status?.ahead ?? 0);
const behind = computed(() => store.status?.behind ?? 0);

const taskState = computed<"ready" | "busy" | "failed">(() => {
  if (store.task) return "busy";
  return "ready";
});

const syncState = computed<"synced" | "ahead" | "behind" | "none">(() => {
  if (!store.status?.upstream) return "none";
  if (ahead.value > 0) return "ahead";
  if (behind.value > 0) return "behind";
  return "synced";
});

function headTime(unix: number): string {
  // 状态栏最近提交：相对时间（超一天用日期时间）
  return Date.now() / 1000 - unix > 86400 ? formatDateTime(unix) : relativeTime(unix);
}
</script>

<template>
  <div class="statusbar">
    <button class="sb-item branch" :title="t('Current branch, click to manage')" @click="emit('openBranches')">
      <IconGlyph name="branch" :size="14" />
      <span>{{ branch || "—" }}</span>
    </button>
    <span class="sb-sep" />
    <span class="sb-item last-commit" :title="head?.summary ?? ''">
      <span class="sb-label">{{ t("Last Commit") }}:</span>
      <span v-if="head" class="ellipsis">{{ headTime(head.timeUnix) }}</span>
      <span v-else>—</span>
    </span>
    <span class="sb-sep" />
    <span class="sb-item user" :title="t('Git identity for commits')">
      <span class="sb-label">{{ t("Current User") }}:</span>
      <span class="ellipsis">{{ store.identityText || "—" }}</span>
    </span>
    <div class="flex-spacer" />
    <span
      class="sb-item task"
      :class="taskState"
      :title="t('Task status')"
    >
      <span class="dot" />
      <span>{{ taskState === "busy" ? t("Busy") : taskState === "failed" ? t("Failed") : t("Ready") }}</span>
      <span v-if="taskState === 'busy' && store.task" class="ellipsis task-detail">
        {{ t(store.task.stage) }}
        <template v-if="store.task.total > 0">{{ store.task.done }}/{{ store.task.total }}</template>
      </span>
    </span>
    <span class="sb-sep" />
    <button
      v-if="syncState !== 'none'"
      class="sb-item sync"
      :title="
        syncState === 'behind'
          ? t('Behind the remote, click to pull')
          : syncState === 'ahead'
            ? t('Ahead of the remote, click to push')
            : t('In sync with the remote')
      "
      @click="emit('syncClick', syncState === 'behind' ? 'pull' : 'push')"
    >
      <IconGlyph :name="syncState === 'behind' ? 'arrow-down' : syncState === 'ahead' ? 'arrow-up' : 'refresh'" :size="14" />
      <span v-if="syncState === 'synced'">{{ t("Repo Synced") }}</span>
      <span v-else-if="syncState === 'ahead'">{{ t("Ahead") }} ↑{{ ahead }}</span>
      <span v-else>{{ t("Behind") }} ↓{{ behind }}</span>
    </button>
    <span class="sb-sep" />
    <button class="sb-item legend" :title="t('File status color legend')" @click="emit('openLegend')">
      <IconGlyph name="legend" :size="14" />
      <span>{{ t("Legend") }}</span>
    </button>
  </div>
</template>

<style scoped>
.statusbar {
  height: 34px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 12px;
  background: var(--bg-statusbar);
  border-top: 1px solid var(--divider);
  font-size: 12px;
  color: var(--text-secondary);
  flex: none;
}

.sb-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 320px;
  padding: 3px 8px;
  border-radius: var(--radius-sm);
  border: none;
  background: none;
  color: inherit;
  font-size: 12px;
  cursor: default;
}

button.sb-item {
  cursor: pointer;
}

button.sb-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.sb-label {
  color: var(--text-tertiary);
  flex: none;
}

.sb-sep {
  width: 1px;
  height: 16px;
  background: var(--border-strong);
  flex: none;
}

.branch {
  color: var(--accent);
  font-weight: 600;
}

.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--status-staged);
  flex: none;
}

.task.busy .dot {
  background: var(--status-modified);
}

.task.failed .dot {
  background: var(--status-deleted);
}

.task-detail {
  color: var(--text-tertiary);
}

.flex-spacer {
  flex: 1;
}

.sync {
  color: var(--accent);
}

.legend {
  color: var(--text-secondary);
}
</style>
