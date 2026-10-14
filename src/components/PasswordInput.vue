<script setup lang="ts">
/** 密码 / Token 输入框：带明文切换（眼睛图标）；其余属性（placeholder、事件）透传给 input */
import { ref } from "vue";
import { t } from "../i18n";
import IconGlyph from "./IconGlyph.vue";

const model = defineModel<string>({ default: "" });
const visible = ref(false);
</script>

<template>
  <div class="pwd-wrap">
    <input v-model="model" :type="visible ? 'text' : 'password'" v-bind="$attrs" />
    <button
      class="pwd-eye"
      type="button"
      :title="visible ? t('Hide password') : t('Show password')"
      @click="visible = !visible"
    >
      <IconGlyph :name="visible ? 'eye-off' : 'eye'" :size="15" />
    </button>
  </div>
</template>

<style scoped>
.pwd-wrap {
  position: relative;
  display: flex;
  align-items: center;
}

.pwd-wrap input {
  width: 100%;
  padding-right: 30px;
}

.pwd-eye {
  position: absolute;
  right: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  background: none;
  color: var(--text-tertiary);
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.pwd-eye:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}
</style>
