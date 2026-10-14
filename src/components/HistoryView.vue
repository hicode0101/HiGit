<script setup lang="ts">
/** 提交历史（FR-6）：分页加载 + 过滤 + 详情与单文件差异 */
import { computed, ref, watch } from "vue";
import { store, showError } from "../store";
import { api } from "../api";
import type { CommitDetailDto, CommitEntryDto } from "../api/types";
import { t, relativeTime } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const emit = defineEmits<{ openCommitDiff: [oid: string, path: string] }>();

const PAGE = 100;

const entries = ref<CommitEntryDto[]>([]);
const filter = ref("");
const loading = ref(false);
const done = ref(false);
const detail = ref<CommitDetailDto | null>(null);

const filtered = computed(() => {
  const q = filter.value.trim().toLowerCase();
  if (!q) return entries.value;
  return entries.value.filter(
    (e) =>
      e.summary.toLowerCase().includes(q) ||
      e.authorName.toLowerCase().includes(q) ||
      e.id.toLowerCase().startsWith(q),
  );
});

async function loadFirst() {
  entries.value = [];
  done.value = false;
  detail.value = null;
  await loadMore(true);
}

async function loadMore(first = false) {
  if (loading.value || done.value || !store.repoOpen) return;
  loading.value = true;
  try {
    const page = await api.historyPage(PAGE, entries.value.length);
    entries.value.push(...page);
    if (page.length < PAGE) done.value = true;
  } catch (e) {
    if (first) showError(e as Error);
    done.value = true;
  } finally {
    loading.value = false;
  }
}

function onScroll(e: Event) {
  const el = e.target as HTMLElement;
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 120) {
    void loadMore();
  }
}

async function showDetail(e: CommitEntryDto) {
  try {
    detail.value = await api.commitDetail(e.id);
  } catch (err) {
    showError(err as Error);
  }
}

watch(
  () => store.repoPath,
  () => void loadFirst(),
);
watch(
  () => store.tab,
  (tab) => {
    if (tab === "history" && entries.value.length === 0) void loadFirst();
  },
);
void loadFirst();

function fileKindLabel(k: string): string {
  const map: Record<string, string> = {
    added: "New",
    modified: "Modified",
    deleted: "Deleted",
    renamed: "Modified",
  };
  return t(map[k] ?? k);
}
</script>

<template>
  <div class="history-view">
    <div class="history-list" @scroll="onScroll">
      <div class="filter-row">
        <input v-model="filter" :placeholder="t('Filter commits (message / author / hash)')" />
      </div>
      <div class="list-body">
        <div
          v-for="e in filtered"
          :key="e.id"
          class="commit-row"
          :class="{ selected: detail?.id === e.id }"
          @click="showDetail(e)"
        >
          <div class="commit-main">
            <div class="commit-title-row">
              <span class="ellipsis commit-summary">{{ e.summary }}</span>
              <span v-for="r in e.refs" :key="r" class="ref-badge">{{ r }}</span>
            </div>
            <div class="commit-meta">
              <span>{{ e.authorName }}</span>
              <span>·</span>
              <span>{{ relativeTime(e.timeUnix) }}</span>
            </div>
          </div>
          <span class="commit-hash">{{ e.shortId }}</span>
        </div>
        <div v-if="!loading && filtered.length === 0" class="empty-hint">
          {{ store.status?.head ? t("Select a commit to view details") : t("No commits yet") }}
        </div>
        <div v-if="loading" class="empty-hint">…</div>
      </div>
    </div>

    <div class="history-detail">
      <template v-if="detail">
        <div class="detail-head">
          <div class="detail-summary">{{ detail.summary }}</div>
          <div class="detail-meta">
            <span><b>{{ t("Author") }}:</b> {{ detail.authorName }} &lt;{{ detail.authorEmail }}&gt;</span>
            <span><b>{{ t("Date") }}:</b> {{ new Date(detail.timeUnix * 1000).toLocaleString() }}</span>
            <span v-if="detail.parents.length">
              <b>{{ t("Parents") }}:</b> {{ detail.parents.map((p) => p.slice(0, 7)).join(", ") }}
            </span>
            <span class="hash-chip">{{ detail.shortId }}</span>
          </div>
          <pre v-if="detail.message.trim() !== detail.summary" class="detail-message">{{ detail.message.trim() }}</pre>
        </div>
        <div class="detail-files">
          <div class="files-head">{{ t("Changed files") }} · {{ detail.files.length }} {{ t("files") }}</div>
          <div
            v-for="f in detail.files"
            :key="f.path"
            class="file-line"
            @click="emit('openCommitDiff', detail.id, f.path)"
          >
            <IconGlyph name="file" :size="14" />
            <span class="ellipsis file-path">{{ f.oldPath ? `${f.oldPath} → ${f.path}` : f.path }}</span>
            <span class="file-kind" :class="f.kind">{{ fileKindLabel(f.kind) }}</span>
            <span class="add-count">+{{ f.additions }}</span>
            <span class="del-count">−{{ f.deletions }}</span>
          </div>
        </div>
      </template>
      <div v-else class="empty-hint full">{{ t("Select a commit to view details") }}</div>
    </div>
  </div>
</template>

<style scoped>
.history-view {
  display: flex;
  overflow: hidden;
  margin: 0 18px 14px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  background: var(--bg-card);
}

.history-list {
  width: 360px;
  flex: none;
  border-right: 1px solid var(--divider);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.filter-row {
  padding: 10px;
  border-bottom: 1px solid var(--divider);
  flex: none;
}

.filter-row input {
  width: 100%;
}

.list-body {
  overflow-y: auto;
  flex: 1;
}

.commit-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--divider);
  cursor: pointer;
}

.commit-row:hover {
  background: var(--bg-hover);
}

.commit-row.selected {
  background: var(--bg-card-selected);
}

.commit-main {
  flex: 1;
  min-width: 0;
}

.commit-title-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.commit-summary {
  font-size: 13px;
  font-weight: 500;
}

.ref-badge {
  flex: none;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 8px;
  background: var(--accent-subtle);
  color: var(--accent);
  border: 1px solid var(--accent);
}

.commit-meta {
  display: flex;
  gap: 5px;
  font-size: 11px;
  color: var(--text-tertiary);
  margin-top: 2px;
}

.commit-hash {
  flex: none;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-tertiary);
}

.history-detail {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 14px 18px;
}

.detail-summary {
  font-size: 15px;
  font-weight: 600;
}

.detail-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  font-size: 12px;
  color: var(--text-secondary);
  margin: 8px 0;
}

.detail-meta b {
  color: var(--text-tertiary);
  font-weight: 600;
}

.hash-chip {
  font-family: var(--font-mono);
  background: var(--bg-code);
  padding: 1px 8px;
  border-radius: var(--radius-sm);
}

.detail-message {
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg-code);
  border-radius: var(--radius-sm);
  padding: 10px;
  white-space: pre-wrap;
  font-family: inherit;
  margin: 0 0 10px;
}

.files-head {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
  padding: 8px 0 4px;
  border-bottom: 1px solid var(--divider);
}

.file-line {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 4px;
  border-bottom: 1px solid var(--divider);
  font-size: 12px;
  cursor: pointer;
}

.file-line:hover {
  background: var(--bg-hover);
}

.file-path {
  flex: 1;
  min-width: 0;
}

.file-kind {
  flex: none;
  color: var(--text-tertiary);
}

.add-count {
  color: var(--status-staged);
  font-family: var(--font-mono);
  font-size: 11px;
}

.del-count {
  color: var(--status-deleted);
  font-family: var(--font-mono);
  font-size: 11px;
}

.empty-hint {
  text-align: center;
  color: var(--text-tertiary);
  padding: 30px 0;
}

.empty-hint.full {
  padding-top: 30vh;
}
</style>
