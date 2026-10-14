<script setup lang="ts">
/** 状态图例（FR-3.5） / 关于 / 快捷键 / 身份引导 / 进度 / 轻提示 */
import { computed, ref } from "vue";
import { store } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";
import IconGlyph from "./IconGlyph.vue";
import appIcon from "../assets/icon.png";

// ---------- 图例 ----------
const legendItems = [
  { kind: "clean", icon: "check", label: "Unmodified", desc: "Unmodified" },
  { kind: "modified", icon: "warn", label: "Modified", desc: "Modified" },
  { kind: "staged", icon: "check", label: "Staged", desc: "Staged" },
  { kind: "untracked", icon: "plus", label: "New", desc: "Untracked" },
  { kind: "deleted", icon: "minus", label: "Deleted", desc: "Deleted" },
  { kind: "conflicted", icon: "warn", label: "Conflicted", desc: "Conflicted" },
  { kind: "ignored", icon: "close", label: "Ignored", desc: "Ignored" },
];

// ---------- 身份引导（FR-4.2） ----------
const idName = ref("");
const idEmail = ref("");
const applyGlobal = ref(true);

async function saveIdentity() {
  if (!idName.value.trim() || !idEmail.value.trim()) return;
  try {
    await api.setIdentity(applyGlobal.value ? "global" : "repo", {
      name: idName.value.trim(),
      email: idEmail.value.trim(),
    });
    store.ui.identityGuide = false;
    const { refreshIdentity } = await import("../store");
    await refreshIdentity();
  } catch (e) {
    store.ui.error = { message: (e as Error).message ?? String(e), raw: "" };
  }
}

// ---------- 进度 ----------
const progress = computed(() => store.ui.progress);

async function cancelProgress() {
  const jobId = store.ui.progress?.jobId;
  if (jobId) await api.cancelJob(jobId).catch(() => undefined);
}

// ---------- 诊断 ----------
const version = computed(() => store.version);
const siteUrl = "https://github.com/hicode0101/HiTools";

function openSite() {
  void api.openUrl(siteUrl).catch((e) => {
    store.ui.error = { message: (e as Error).message ?? String(e), raw: "" };
  });
}
</script>

<template>
  <!-- 状态图例 -->
  <DialogBase :title="t('Status Legend')" :width="380" v-if="store.ui.legend" @close="store.ui.legend = false">
    <div class="legend-list">
      <div v-for="it in legendItems" :key="it.kind" class="legend-row">
        <span class="badge" :class="it.kind"><IconGlyph :name="it.icon" :size="12" /></span>
        <span class="lg-label">{{ t(it.label) }}</span>
      </div>
    </div>
    <template #footer>
      <button class="btn primary" @click="store.ui.legend = false">{{ t("Close") }}</button>
    </template>
  </DialogBase>

  <!-- 关于 -->
  <DialogBase :title="t('About HiGit')" :width="420" v-if="store.ui.about" @close="store.ui.about = false">
    <div class="about">
      <img :src="appIcon" class="about-logo" alt="HiGit" draggable="false" />
      <div class="about-name">HiGit</div>
      <div class="about-ver">v{{ version }}</div>
      <div class="about-desc">{{ t("A Git client built for engineers.") }}</div>
      <div class="about-meta">
        <div class="meta-row">
          <span class="meta-label">{{ t("Website") }}:</span>
          <span class="meta-link" @click="openSite">{{ siteUrl }}</span>
        </div>
        <div class="meta-row">
          <span class="meta-label">{{ t("Author") }}:</span>
          <span>hicode0101(微信)</span>
        </div>
      </div>
    </div>
    <template #footer>
      <button class="btn primary" @click="store.ui.about = false">{{ t("Close") }}</button>
    </template>
  </DialogBase>

  <!-- 快捷键一览 -->
  <DialogBase :title="t('Keyboard Shortcuts')" :width="440" v-if="store.ui.shortcuts" @close="store.ui.shortcuts = false">
    <div class="sc-row"><span>{{ t("New Repository") }}</span><kbd>Ctrl+N</kbd></div>
    <div class="sc-row"><span>{{ t("Clone Repository") }}</span><kbd>Ctrl+Shift+C</kbd></div>
    <div class="sc-row"><span>{{ t("Refresh") }}</span><kbd>F5</kbd></div>
    <div class="sc-row"><span>{{ t("Toggle Sidebar") }}</span><kbd>Ctrl+B</kbd></div>
    <div class="sc-row"><span>{{ t("Commit") }}</span><kbd>Ctrl+Enter</kbd></div>
    <div class="sc-row"><span>{{ t("Settings") }}</span><kbd>Ctrl+,</kbd></div>
    <div class="sc-row"><span>{{ t("Close dialog / diff") }}</span><kbd>Esc</kbd></div>
    <template #footer>
      <button class="btn primary" @click="store.ui.shortcuts = false">{{ t("Close") }}</button>
    </template>
  </DialogBase>

  <!-- Git 身份引导（FR-4.2） -->
  <DialogBase
    :title="t('Git Identity')"
    :width="440"
    :close-on-mask="false"
    v-if="store.ui.identityGuide"
    @close="store.ui.identityGuide = false"
  >
    <div class="hint">{{ t("Git identity is not configured (user.name / user.email)") }}</div>
    <div class="form-row">
      <label>{{ t("Your Name") }}</label>
      <input v-model="idName" />
    </div>
    <div class="form-row">
      <label>{{ t("Email") }}</label>
      <input v-model="idEmail" />
    </div>
    <label class="check">
      <input v-model="applyGlobal" type="checkbox" />
      <span>{{ t("Apply globally") }}</span>
    </label>
    <template #footer>
      <button class="btn" @click="store.ui.identityGuide = false">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!idName.trim() || !idEmail.trim()" @click="saveIdentity">
        {{ t("Save") }}
      </button>
    </template>
  </DialogBase>

  <!-- 长任务进度（FR-5.10） -->
  <DialogBase
    :title="progress?.title ?? ''"
    :width="420"
    :close-on-mask="false"
    v-if="progress"
    @close="() => {}"
  >
    <div class="progress-stage">{{ progress.stage || "…" }}</div>
    <div class="bar">
      <div class="bar-fill" :style="{ width: progress.percent + '%' }" />
    </div>
    <template #footer>
      <button class="btn" @click="cancelProgress">{{ t("Cancel") }}</button>
    </template>
  </DialogBase>
</template>

<style scoped>
.legend-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.legend-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.badge {
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
}

.badge.clean {
  color: var(--status-clean);
}

.badge.modified {
  color: var(--status-modified);
}

.badge.staged,
.badge.untracked {
  color: var(--status-staged);
}

.badge.deleted {
  color: var(--status-deleted);
}

.badge.conflicted {
  color: var(--status-conflict);
}

.badge.ignored {
  color: var(--status-ignored);
}

.lg-label {
  font-size: 13px;
}

.about {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 10px 0;
}

.about-logo {
  width: 56px;
  height: 56px;
  margin-bottom: 4px;
  user-select: none;
}

.about-name {
  font-size: 20px;
  font-weight: 700;
}

.about-ver {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--accent);
}

.about-desc {
  font-size: 12px;
  color: var(--text-tertiary);
}

.about-meta {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 2px;
}

.meta-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-secondary);
}

.meta-label {
  color: var(--text-tertiary);
  flex: none;
}

.meta-link {
  color: var(--accent);
  cursor: pointer;
  word-break: break-all;
}

.meta-link:hover {
  text-decoration: underline;
}

.sc-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 4px;
  border-bottom: 1px solid var(--divider);
  font-size: 13px;
}

kbd {
  font-family: var(--font-mono);
  font-size: 11px;
  background: var(--bg-code);
  border: 1px solid var(--border-color);
  border-bottom-width: 2px;
  border-radius: var(--radius-sm);
  padding: 2px 8px;
}

.hint {
  font-size: 13px;
  color: var(--status-modified);
  margin-bottom: 14px;
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 5px;
  margin-bottom: 12px;
}

.form-row label {
  font-size: 12px;
  color: var(--text-secondary);
}

.check {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  cursor: pointer;
}

.progress-stage {
  font-size: 13px;
  color: var(--text-secondary);
  margin-bottom: 10px;
}

.bar {
  height: 6px;
  background: var(--bg-active);
  border-radius: 3px;
  overflow: hidden;
}

.bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width 0.2s;
}
</style>
