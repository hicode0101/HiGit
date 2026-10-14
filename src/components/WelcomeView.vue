<script setup lang="ts">
/** 欢迎页（FR-5.5 空状态）：新建 / 克隆 / 添加本地仓库 三入口卡片 */
import { store } from "../store";
import { addLocalRepoAction } from "../tasks";
import { t } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const cards = [
  { name: "new", icon: "new-repo", title: t("New Repository"), desc: t("Create a repository from a local directory") },
  { name: "clone", icon: "clone", title: t("Clone Repository"), desc: t("Clone an existing repository from a URL") },
  { name: "add", icon: "folder-open", title: t("Add Local Repository"), desc: t("Open a repository that already exists on disk") },
];

function onAction(name: string) {
  if (name === "new") store.ui.newRepo = true;
  else if (name === "clone") store.ui.clone = true;
  else void addLocalRepoAction();
}
</script>

<template>
  <div class="welcome">
    <div class="brand">
      <svg width="64" height="64" viewBox="0 0 24 24" fill="none">
        <circle cx="12" cy="12" r="10" fill="var(--accent)" />
        <circle cx="8.2" cy="7.8" r="1.8" fill="#fff" />
        <circle cx="8.2" cy="16.2" r="1.8" fill="#fff" />
        <circle cx="16" cy="10" r="1.8" fill="#fff" />
        <path d="M8.2 9.6v4.8M16 11.8c0 2.6-3.2 3.4-5.4 3.7" stroke="#fff" stroke-width="1.4" stroke-linecap="round" fill="none" />
      </svg>
      <h1>HiGit</h1>
      <p>{{ t("A zero-dependency, cross-platform Git client with a Fluent UI") }}</p>
    </div>
    <div class="cards">
      <button v-for="c in cards" :key="c.name" class="card" @click="onAction(c.name)">
        <IconGlyph :name="c.icon" :size="26" />
        <div class="card-title">{{ c.title }}</div>
        <div class="card-desc">{{ c.desc }}</div>
      </button>
    </div>
  </div>
</template>

<style scoped>
.welcome {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 44px;
  overflow: auto;
}

.brand {
  text-align: center;
}

.brand h1 {
  font-size: 26px;
  margin: 12px 0 6px;
}

.brand p {
  color: var(--text-tertiary);
  margin: 0;
  font-size: 13px;
}

.cards {
  display: flex;
  gap: 18px;
}

.card {
  width: 210px;
  padding: 26px 18px;
  background: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-card);
  cursor: pointer;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  color: var(--text-primary);
}

.card:hover {
  border-color: var(--accent);
  background: var(--accent-subtle);
}

.card :deep(svg) {
  color: var(--accent);
}

.card-title {
  font-size: 14px;
  font-weight: 600;
}

.card-desc {
  font-size: 12px;
  color: var(--text-tertiary);
  text-align: center;
  line-height: 1.5;
}
</style>
