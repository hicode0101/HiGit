<script setup lang="ts">
/** 面包屑导航（FR-2.3）：仓库：{repo} > {子路径…}，点击任意层级跳转；仓库名旁可打开仓库设置 */
import { computed } from "vue";
import { store, navigate } from "../store";
import { t } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const crumbs = computed(() => {
  const parts = store.currentDir ? store.currentDir.split("/") : [];
  return parts;
});

const repoId = computed(() => store.repos.find((r) => r.path === store.repoPath)?.id ?? "");
</script>

<template>
  <div class="breadcrumb">
    <span class="crumb-repo">
      <span class="check-dot"><IconGlyph name="check" :size="11" /></span>
      <span class="crumb-label">{{ t("Repo") }}</span>
    </span>
    <button class="crumb-link" @click="navigate('')">{{ store.repoName }}</button>
    <button
      v-if="repoId"
      class="repo-edit"
      :title="t('Repository Settings')"
      @click="store.ui.repoSettingsId = repoId"
    >
      <IconGlyph name="repo-settings" :size="15" />
    </button>
    <template v-for="(part, i) in crumbs" :key="i">
      <span class="crumb-sep"><IconGlyph name="chevronRight" :size="12" /></span>
      <button
        class="crumb-link"
        :class="{ current: i === crumbs.length - 1 }"
        @click="navigate(crumbs.slice(0, i + 1).join('/'))"
      >
        {{ part }}
      </button>
    </template>
  </div>
</template>

<style scoped>
.breadcrumb {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 8px 26px;
  font-size: 13px;
  flex: none;
  overflow: hidden;
}

.check-dot {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--accent);
  color: #fff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
}

.crumb-repo {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
  margin: 0 4px;
}

.crumb-label {
  font-weight: 600;
  color: var(--text-primary);
}

.crumb-link {
  border: none;
  background: none;
  padding: 3px 6px;
  font-size: 13px;
  color: var(--text-secondary);
  cursor: pointer;
  border-radius: var(--radius-sm);
  max-width: 240px;
}

.crumb-link:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.crumb-link.current {
  color: var(--text-tertiary);
  cursor: default;
}

.crumb-sep {
  color: var(--text-tertiary);
  display: inline-flex;
}

.repo-edit {
  border: none;
  background: none;
  color: var(--text-tertiary);
  cursor: pointer;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-left: 2px;
}

.repo-edit:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
</style>
