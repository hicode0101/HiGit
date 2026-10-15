<script setup lang="ts">
/** 新建仓库对话框（FR-1.1）：目录 + 名称 + README / .gitignore 语言模板 */
import { computed, ref } from "vue";
import { store, openRepo, showError } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";
import { gitignoreStacks } from "../gitignore";
import { joinNative } from "../path";

const emit = defineEmits<{ close: [] }>();

const parentDir = ref("");
const name = ref("");
const withReadme = ref(true);
const withGitignore = ref(true);
const stack = ref("General");
const busy = ref(false);

const canCreate = computed(() => parentDir.value.trim() && name.value.trim() && !busy.value);

async function chooseDir() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const dir = await open({ directory: true, title: t("Parent directory") });
  if (dir && typeof dir === "string") parentDir.value = dir;
}

async function create() {
  if (!canCreate.value) return;
  const target = joinNative(parentDir.value, name.value);
  // 目标目录非空校验
  try {
    const st = await api.destStatus(target);
    if (st === "nonempty") {
      store.ui.error = { message: t("Destination directory already exists and is not empty"), raw: "" };
      return;
    }
  } catch {
    /* 检查失败不阻断，由后端兜底 */
  }
  busy.value = true;
  try {
    await api.newRepo({
      parentDir: parentDir.value,
      name: name.value,
      withReadme: withReadme.value,
      withGitignore: withGitignore.value,
      gitignoreStack: stack.value,
    });
    emit("close");
    await openRepo(target);
  } catch (e) {
    showError(e as Error);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <DialogBase :title="t('New Repository')" :width="520" @close="emit('close')">
    <div class="form-row">
      <label>{{ t("Parent directory") }}</label>
      <div class="form-inline">
        <input v-model="parentDir" :placeholder="t('Choose a path or enter one manually')" />
        <button class="btn" @click="chooseDir">{{ t("Choose…") }}</button>
      </div>
    </div>
    <div class="form-row">
      <label>{{ t("Directory Name") }}</label>
      <input v-model="name" :placeholder="t('Please enter a repository name')" />
    </div>
    <div class="form-row">
      <label class="check-line">
        <input v-model="withReadme" type="checkbox" />
        <span>{{ t("Initialize with README") }}</span>
      </label>
    </div>
    <div class="form-row">
      <label class="check-line">
        <input v-model="withGitignore" type="checkbox" />
        <span>{{ t("Add .gitignore") }}</span>
      </label>
    </div>
    <div v-if="withGitignore" class="form-row">
      <label>{{ t(".gitignore template") }}</label>
      <select v-model="stack">
        <option v-for="s in gitignoreStacks" :key="s" :value="s">{{ s }}</option>
      </select>
    </div>

    <template #footer>
      <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
      <button class="btn primary" :disabled="!canCreate" @click="create">
        {{ t("Create Repository") }}
      </button>
    </template>
  </DialogBase>
</template>

<style scoped>
.check-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: var(--text-primary);
  cursor: pointer;
}

select {
  font-family: inherit;
  font-size: 13px;
  padding: 6px 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  background: var(--bg-input);
  color: var(--text-primary);
}
</style>
