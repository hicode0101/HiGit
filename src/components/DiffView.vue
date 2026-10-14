<script setup lang="ts">
/** 差异查看对话框（FR-8.1/8.3/8.4）：工作区/暂存区切换 + 统一/并排视图 + 二进制/超大文件降级 */
import { computed, ref, watch } from "vue";
import { store, showError } from "../store";
import { api } from "../api";
import type { DiffDto } from "../api/types";
import { t } from "../i18n";
import DialogBase from "./DialogBase.vue";

const props = defineProps<{
  /** { path, spec } = 工作区差异；{ oid, path } = 提交内差异 */
  work: { path: string; spec: "workdir" | "staged" } | null;
  commit: { oid: string; path: string } | null;
}>();

const emit = defineEmits<{ close: [] }>();

const diff = ref<DiffDto | null>(null);
const loading = ref(false);
const view = ref<"unified" | "split">("unified");
const spec = ref<"workdir" | "staged">("workdir");

const title = computed(() => {
  const path = props.work?.path ?? props.commit?.path ?? "";
  const prefix = props.commit ? `${props.commit.oid.slice(0, 7)} · ` : "";
  return `${prefix}${path}`;
});

async function load() {
  diff.value = null;
  loading.value = true;
  try {
    if (props.work) {
      spec.value = props.work.spec;
      diff.value = await api.diffFile(props.work.path, props.work.spec);
    } else if (props.commit) {
      diff.value = await api.commitFileDiff(props.commit.oid, props.commit.path);
    }
  } catch (e) {
    showError(e as Error);
    emit("close");
  } finally {
    loading.value = false;
  }
}

watch(() => [props.work, props.commit], load, { immediate: true });

async function switchSpec(s: "workdir" | "staged") {
  if (!props.work) return;
  try {
    spec.value = s;
    diff.value = await api.diffFile(props.work.path, s);
  } catch (e) {
    showError(e as Error);
  }
}

interface Line {
  origin: string;
  text: string;
  left: string;
  right: string;
  cls: string;
}

const unifiedLines = computed<Line[]>(() => {
  const lines = diff.value?.lines ?? [];
  return lines.map((l) => ({
    origin: l.origin,
    text: l.text,
    left: "",
    right: "",
    cls: l.origin,
  }));
});

/** 并排视图：相邻"删除块+新增块"配对（FR-8.3） */
const splitLines = computed<Line[]>(() => {
  const src = diff.value?.lines ?? [];
  const out: Line[] = [];
  let i = 0;
  const mk = (origin: string, left: string, right: string, cls: string): Line => ({
    origin,
    text: "",
    left,
    right,
    cls,
  });
  while (i < src.length) {
    const l = src[i];
    if (l.origin === "del") {
      const dels: string[] = [];
      while (i < src.length && src[i].origin === "del") dels.push(src[i++].text);
      const adds: string[] = [];
      while (i < src.length && src[i].origin === "add") adds.push(src[i++].text);
      const n = Math.max(dels.length, adds.length);
      for (let k = 0; k < n; k++) {
        const row = mk("pair", dels[k] ?? "", adds[k] ?? "", "pair");
        row.cls =
          dels[k] && adds[k] ? "pair-mod" : dels[k] ? "pair-del" : "pair-add";
        out.push(row);
      }
    } else if (l.origin === "add") {
      out.push(mk("pair", "", l.text, "pair-add"));
      i++;
    } else {
      out.push(mk(l.origin, l.text, l.text, l.origin));
      i++;
    }
  }
  return out;
});

const lines = computed<Line[]>(() => (view.value === "split" ? splitLines.value : unifiedLines.value));

const placeholder = computed(() => {
  if (!diff.value) return "";
  if (diff.value.isBinary) return t("Binary file — differences not shown");
  if (diff.value.tooLarge) return t("File is too large to display (over 1 MB)");
  if (diff.value.truncated) return t("Differences truncated (too many lines)");
  return "";
});
</script>

<template>
  <DialogBase :title="title" :width="960" @close="emit('close')">
    <div class="diff-toolbar">
      <template v-if="work">
        <button class="chip" :class="{ on: spec === 'workdir' }" @click="switchSpec('workdir')">
          {{ t("Working") }}
        </button>
        <button class="chip" :class="{ on: spec === 'staged' }" @click="switchSpec('staged')">
          {{ t("Staged") }}
        </button>
      </template>
      <div class="flex-spacer" />
      <span class="stats"><span class="add-count">+{{ diff?.additions ?? 0 }}</span> <span class="del-count">−{{ diff?.deletions ?? 0 }}</span></span>
      <button class="chip" :class="{ on: view === 'unified' }" @click="view = 'unified'">
        {{ t("Unified") }}
      </button>
      <button class="chip" :class="{ on: view === 'split' }" @click="view = 'split'">
        {{ t("Side by side") }}
      </button>
    </div>

    <div v-if="loading" class="diff-empty">…</div>
    <div v-else-if="placeholder" class="diff-empty">{{ placeholder }}</div>
    <div v-else class="diff-body">
      <div v-if="view === 'unified'">
        <div v-for="(l, i) in lines" :key="i" class="dline" :class="l.cls">
          <span class="sign">{{
            l.origin === "add" ? "+" : l.origin === "del" ? "−" : l.origin === "hunk" ? "" : " "
          }}</span>
          <span class="dtext">{{ l.text }}</span>
        </div>
      </div>
      <div v-else class="split">
        <div v-for="(l, i) in lines" :key="i" class="srow" :class="l.cls">
          <span class="scell left"><span class="dtext">{{ l.left }}</span></span>
          <span class="scell right"><span class="dtext">{{ l.right }}</span></span>
        </div>
      </div>
    </div>
  </DialogBase>
</template>

<style scoped>
.diff-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
  flex: none;
}

.chip {
  border: 1px solid var(--border-color);
  background: var(--bg-card);
  color: var(--text-secondary);
  font-size: 12px;
  padding: 4px 12px;
  border-radius: 12px;
  cursor: pointer;
}

.chip.on {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.chip:hover:not(.on) {
  background: var(--bg-hover);
}

.flex-spacer {
  flex: 1;
}

.stats {
  font-family: var(--font-mono);
  font-size: 12px;
  margin-right: 8px;
}

.add-count {
  color: var(--status-staged);
}

.del-count {
  color: var(--status-deleted);
}

.diff-body {
  font-family: var(--font-mono);
  font-size: 12px;
  background: var(--bg-code);
  border: 1px solid var(--divider);
  border-radius: var(--radius-md);
  overflow: auto;
  max-height: 56vh;
  user-select: text;
}

.dline {
  display: flex;
  white-space: pre;
  line-height: 19px;
  padding: 0 10px;
}

.sign {
  width: 16px;
  flex: none;
  user-select: none;
  color: var(--text-tertiary);
}

.dline.add {
  background: rgba(15, 123, 15, 0.16);
}

.dline.add .sign {
  color: var(--status-staged);
}

.dline.del {
  background: rgba(196, 43, 28, 0.14);
}

.dline.del .sign {
  color: var(--status-deleted);
}

.dline.hunk {
  color: var(--accent);
  background: var(--accent-subtle);
  font-weight: 600;
}

.split {
  display: flex;
  flex-direction: column;
}

.srow {
  display: flex;
}

.scell {
  flex: 1;
  min-width: 0;
  white-space: pre;
  line-height: 19px;
  padding: 0 10px;
}

.scell.left {
  border-right: 1px solid var(--divider);
}

.srow.pair-del .scell.left {
  background: rgba(196, 43, 28, 0.14);
}

.srow.pair-add .scell.right {
  background: rgba(15, 123, 15, 0.16);
}

.srow.pair-mod .scell.left {
  background: rgba(196, 43, 28, 0.14);
}

.srow.pair-mod .scell.right {
  background: rgba(15, 123, 15, 0.16);
}

.dtext {
  overflow: hidden;
  text-overflow: clip;
}

.diff-empty {
  text-align: center;
  color: var(--text-tertiary);
  padding: 60px 0;
}
</style>
