<script setup lang="ts">
/** 线性 SVG 图标字典（Fluent 风格，currentColor 着色） */
const props = withDefaults(defineProps<{ name: string; size?: number }>(), { size: 16 });

const ICONS: Record<string, string> = {
  // 仓库 / 工具栏
  "new-repo":
    '<path d="M3 5.5A1.5 1.5 0 0 1 4.5 4h4l1.6 2H19a1.5 1.5 0 0 1 1.5 1.5V10M3 5.5V17A1.5 1.5 0 0 0 4.5 18.5H10"/><circle cx="17" cy="15" r="4.2"/><path d="M17 13v4M15 15h4"/>',
  clone:
    '<path d="M11 4h6a2 2 0 0 1 2 2v6"/><path d="M5 8h6a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2Z"/><path d="M8 14.5l2 2 4-4"/>',
  "folder-open":
    '<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l1.8 2.2H19a1.5 1.5 0 0 1 1.5 1.6l-.5 8A1.5 1.5 0 0 1 18.5 18H4.2A1.2 1.2 0 0 1 3 16.8Z"/>',
  commit:
    '<circle cx="12" cy="12" r="3.6"/><path d="M3 12h5.4M15.6 12H21"/>',
  "arrow-up":
    '<path d="M12 19V5M6 11l6-6 6 6"/>',
  "arrow-down":
    '<path d="M12 5v14M6 13l6 6 6-6"/>',
  fetch:
    '<path d="M20 12a8 8 0 1 0-2.3 5.6"/><path d="M20 12v5h-5"/>',
  refresh:
    '<path d="M20 12a8 8 0 1 1-2.3-5.6M20 4v5h-5"/>',
  branch:
    '<circle cx="6" cy="5.5" r="2.2"/><circle cx="6" cy="18.5" r="2.2"/><circle cx="18" cy="7.5" r="2.2"/><path d="M6 7.7v8.6M18 9.7c0 3.3-3 4.5-6 4.8-2 .2-4-.3-4-.3"/>',
  moon: '<path d="M20 13.5A8 8 0 0 1 10.5 4 8 8 0 1 0 20 13.5Z"/>',
  globe:
    '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.3 3.8 5.2 3.8 8.5s-1.3 6.2-3.8 8.5c-2.5-2.3-3.8-5.2-3.8-8.5S9.5 5.8 12 3.5Z"/>',
  folder:
    '<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l1.8 2H19a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 19 19H4.5A1.5 1.5 0 0 1 3 17.5Z"/>',
  "folder-git":
    '<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l1.8 2H19a1.5 1.5 0 0 1 1.5 1.5v9A1.5 1.5 0 0 1 19 19H4.5A1.5 1.5 0 0 1 3 17.5Z"/><circle cx="12" cy="13" r="2.6"/>',
  file:
    '<path d="M6 3.5h7L18.5 9v11a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5v-16a.5.5 0 0 1 .5-.5Z"/><path d="M13 3.5V9h5.5"/>',
  "file-code":
    '<path d="M6 3.5h7L18.5 9v11a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5v-16a.5.5 0 0 1 .5-.5Z"/><path d="M13 3.5V9h5.5"/><path d="M10 12.5 8 14.5l2 2M14 12.5l2 2-2 2"/>',
  "file-doc":
    '<path d="M6 3.5h7L18.5 9v11a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5v-16a.5.5 0 0 1 .5-.5Z"/><path d="M13 3.5V9h5.5"/><path d="M8.5 13h7M8.5 16h7"/>',
  "file-config":
    '<path d="M6 3.5h7L18.5 9v11a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5v-16a.5.5 0 0 1 .5-.5Z"/><path d="M13 3.5V9h5.5"/><circle cx="12" cy="14.5" r="2"/><path d="M12 11.2v1.3M12 16.5v1.3M9.4 13l1.1.7M13.5 15.8l1.1.7M9.4 16l1.1-.7M13.5 13.2l1.1-.7"/>',
  "file-image":
    '<path d="M6 3.5h7L18.5 9v11a.5.5 0 0 1-.5.5H6a.5.5 0 0 1-.5-.5v-16a.5.5 0 0 1 .5-.5Z"/><path d="M13 3.5V9h5.5"/><circle cx="10" cy="13.5" r="1.4"/><path d="M8.5 17.5l2.5-2.5 2 2 1.5-1.5 2 2"/>',
  close: '<path d="M5.5 5.5l13 13M18.5 5.5l-13 13"/>',
  minus: '<path d="M5.5 12h13"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  warn: '<path d="M12 3.5 22 20H2L12 3.5Z"/><path d="M12 9.5v5"/><circle cx="12" cy="17.2" r="0.4" fill="currentColor"/>',
  chevronRight: '<path d="M9 5.5l6.5 6.5L9 18.5"/>',
  chevronDown: '<path d="M5.5 9l6.5 6.5L18.5 9"/>',
  info: '<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5"/><circle cx="12" cy="7.8" r="0.5" fill="currentColor"/>',
  settings:
    '<circle cx="12" cy="12" r="3"/><path d="M12 2.8 13.5 5h2.8l1 2.4 2.4 1.2-.5 2.8 1.8 2.2-1.8 2.2.5 2.8-2.4 1.2-1 2.4h-2.8L12 21.2 10.5 19H7.7l-1-2.4-2.4-1.2.5-2.8L3 10.4 4.8 8.2 4.3 5.4l2.4-1.2 1-2.4h2.8L12 2.8Z"/>',
  terminal:
    '<rect x="3" y="4.5" width="18" height="15" rx="1.5"/><path d="M7 9.5l3 3-3 3M12.5 15.5H17"/>',
  user: '<circle cx="12" cy="8" r="3.8"/><path d="M4.5 20a7.5 7.5 0 0 1 15 0"/>',
  logout: '<path d="M14 4H6.5A1.5 1.5 0 0 0 5 5.5v13A1.5 1.5 0 0 0 6.5 20H14M10.5 12H21M17.5 8.5 21 12l-3.5 3.5"/>',
  trash: '<path d="M4.5 6.5h15M9.5 6V4.5h5V6M7 6.5l.8 13a1 1 0 0 0 1 .9h6.4a1 1 0 0 0 1-.9l.8-13M10 10v6.5M14 10v6.5"/>',
  edit: '<path d="M4 20h4.5L20 8.5a2.1 2.1 0 0 0-3-3L5.5 17 4 20Z"/>',
  "repo-settings":
    '<ellipse cx="9" cy="6" rx="5" ry="2.2"/><path d="M4 6v10.5M14 6v9.5M4 11c0 1.2 2.25 2.2 5 2.2s5-1 5-2.2M4 16.5c0 1.2 2.25 2.2 5 2.2 1 0 2-.1 2.9-.35"/><circle cx="17.6" cy="17.3" r="3.1"/><path d="M20.7 17.3h1.2M19.2 20l.6 1M16 20l-.6 1M14.5 17.3h-1.2M16 14.6l-.6-1M19.2 14.6l.6-1"/><circle cx="17.6" cy="17.3" r="0.9" fill="currentColor" stroke="none"/>',
  legend: '<path d="M4 6h2M4 12h2M4 18h2M9 6h11M9 12h11M9 18h11"/>',
  restore: '<path d="M4 8h5V3M4 8a8.5 8.5 0 1 1-1 6"/>',
  sort: '<path d="M7 5v14M7 19l-3-3M7 19l3-3M17 19V5M17 5l-3 3M17 5l3 3"/>',
  eye: '<path d="M2.5 12S6 5.8 12 5.8 21.5 12 21.5 12 18 18.2 12 18.2 2.5 12 2.5 12Z"/><circle cx="12" cy="12" r="2.8"/>',
  "eye-off":
    '<path d="M4 4l16 16"/><path d="M9.9 5.2A9.4 9.4 0 0 1 12 5c6 0 9.5 7 9.5 7a17.6 17.6 0 0 1-3.1 3.9M6.7 6.7A16.8 16.8 0 0 0 2.5 12S6 19 12 19a9.4 9.4 0 0 0 3.3-.6"/><path d="M9.9 9.9a2.8 2.8 0 0 0 4 4"/>',
};

const inner = () => ICONS[props.name] ?? ICONS.file;
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    v-html="inner()"
  />
</template>
