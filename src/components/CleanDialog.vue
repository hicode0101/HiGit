<script setup lang="ts">
/** 清理对话框（工作区右键，git clean 语义，TortoiseGit 风格）：清理类型 / 目录 / 回收站 / 试运行；
 * 作用于整个工作区；试运行在对话框内列出将被清理的路径，执行后刷新状态并关闭 */
import { computed, ref } from "vue";
import { store, showError, toast, refreshStatus, refreshRepos } from "../store";
import { api } from "../api";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

const emit = defineEmits<{ close: [] }>();

const mode = ref<"all" | "untracked" | "ignored">("all");
const dirs = ref(true);
const forceGitDirs = ref(false);
const noRecycleBin = ref(false);
const dryRun = ref(false);
const busy = ref(false);

/** 试运行结果（非空 = 切换到结果视图，仅可关闭） */
const result = ref<string[] | null>(null);

const modes = computed(() => [
  { value: "all" as const, label: t("Remove all untracked files (-fx)") },
  { value: "untracked" as const, label: t("Remove non-ignored untracked files (-f)") },
  { value: "ignored" as const, label: t("Remove ignored files (-fX)") },
]);

function onDirsChange() {
  if (!dirs.value) forceGitDirs.value = false;
}

async function run() {
  if (busy.value) return;
  busy.value = true;
  try {
    const o = await api.cleanWorktree({
      mode: mode.value,
      dirs: dirs.value,
      forceGitDirs: forceGitDirs.value,
      dryRun: dryRun.value,
      noRecycleBin: noRecycleBin.value,
    });
    if (dryRun.value) {
      result.value = o.paths;
      return;
    }
    if (o.paths.length === 0 && o.failed.length === 0) {
      toast(t("Nothing to clean"));
    } else {
      toast(t("Cleaned {0} items", { 0: o.paths.length }));
      if (o.failed.length > 0) {
        store.ui.error = {
          message: `${t("The following files could not be cleaned:")}\n${o.failed
            .map((f) => `${f.path} (${f.reason})`)
            .join("\n")}`,
          raw: "",
        };
      }
      void Promise.all([refreshStatus(), refreshRepos()]);
    }
    emit("close");
  } catch (e) {
    showError(e as Error);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <DialogBase :title="t('Clean')" :width="480" @close="emit('close')">
    <template v-if="!result">
      <div class="group-box">
        <div class="group-title">{{ t("Clean Type") }}</div>
        <label v-for="m in modes" :key="m.value" class="radio-line">
          <input v-model="mode" type="radio" :value="m.value" />
          <span>{{ m.label }}</span>
        </label>
      </div>

      <label class="check-line">
        <input v-model="dirs" type="checkbox" @change="onDirsChange" />
        <span>{{ t("Remove untracked directories (-d)") }}</span>
      </label>
      <label class="check-line nested">
        <input v-model="forceGitDirs" type="checkbox" :disabled="!dirs" />
        <span>{{ t("Remove unmanaged directories with .git folder (-f)") }}</span>
      </label>

      <label class="check-line">
        <input v-model="noRecycleBin" type="checkbox" />
        <span>{{ t("Do not use recycle bin") }}</span>
      </label>
      <label class="check-line">
        <input v-model="dryRun" type="checkbox" />
        <span>{{ t("Dry run") }}</span>
      </label>

      <div class="attention">{{ t("Attention: This command affects the whole working tree!") }}</div>
    </template>

    <template v-else>
      <div class="group-title">{{ t("The following files would be cleaned:") }}</div>
      <div class="result-box">
        <div v-for="p in result" :key="p" class="result-line">{{ p }}</div>
        <div v-if="result.length === 0" class="result-line dim">{{ t("Nothing to clean") }}</div>
      </div>
    </template>

    <template #footer>
      <template v-if="result">
        <button class="btn primary" @click="emit('close')">{{ t("Close") }}</button>
      </template>
      <template v-else>
        <button class="btn" @click="emit('close')">{{ t("Cancel") }}</button>
        <button class="btn primary" :disabled="busy" @click="run">{{ t("OK") }}</button>
      </template>
    </template>
  </DialogBase>
</template>

<style scoped>
.group-box {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--bg-card);
  padding: 10px 12px;
  margin-bottom: 12px;
}

.group-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.4px;
  margin-bottom: 8px;
}

.radio-line,
.check-line {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  cursor: pointer;
  margin-bottom: 8px;
}

.check-line:last-of-type {
  margin-bottom: 0;
}

.check-line.nested {
  margin-left: 26px;
  font-size: 12px;
  color: var(--text-secondary);
}

.attention {
  margin-top: 12px;
  font-size: 12px;
  color: var(--status-modified);
}

.result-box {
  max-height: 40vh;
  overflow-y: auto;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-sm);
  background: var(--bg-card);
  padding: 8px 10px;
  font-family: ui-monospace, Consolas, monospace;
  font-size: 12px;
  user-select: text;
}

.result-line {
  line-height: 1.6;
  word-break: break-all;
}

.result-line.dim {
  color: var(--text-tertiary);
}
</style>
