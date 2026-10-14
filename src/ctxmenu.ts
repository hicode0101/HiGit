/** 全局右键菜单（图2） */
import { reactive } from "vue";

export interface CtxMenuItem {
  label?: string;
  icon?: string;
  danger?: boolean;
  disabled?: boolean;
  separator?: boolean;
  action?: () => void;
}

interface CtxMenuState {
  visible: boolean;
  x: number;
  y: number;
  items: CtxMenuItem[];
}

export const ctxMenu = reactive<CtxMenuState>({ visible: false, x: 0, y: 0, items: [] });

export function showContextMenu(event: MouseEvent, items: CtxMenuItem[]) {
  event.preventDefault();
  event.stopPropagation();
  ctxMenu.x = event.clientX;
  ctxMenu.y = event.clientY;
  ctxMenu.items = items;
  ctxMenu.visible = true;
}

export function hideContextMenu() {
  ctxMenu.visible = false;
}

export function runCtxAction(item: CtxMenuItem) {
  if (item.disabled || item.separator) return;
  hideContextMenu();
  item.action?.();
}
