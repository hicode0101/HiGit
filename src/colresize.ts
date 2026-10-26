/**
 * 表格式列表的列宽拖拽（工作区 FileTable / 提交对话框 CommitDialog 共用）。
 *
 * 列布局模型：一个弹性吸收列（flex:1）+ 若干固定宽列；分界线左侧为弹性列时传 null。
 * 交互（与资源管理器一致）：拖表头分界线，分界线跟随鼠标——左列加宽、右列变窄、
 * 其余列与分界线不动；拖到任一侧最小宽即停；双击分界线复位默认宽。
 * 列宽按 id 模块级保存：组件卸载（切标签 / 关对话框）后重开仍保留，重启应用恢复默认。
 */
import { computed, reactive, type Ref } from "vue";

/** 各列表的列宽状态（模块级，跨组件卸载保留） */
const widthStore = new Map<string, Record<string, number>>();

export interface ColResizeSpec<C extends string> {
  /** 列宽状态存取键，每处列表唯一 */
  id: string;
  /** 固定宽列名，按界面顺序；弹性列不在此列 */
  cols: C[];
  /** 固定列默认宽 / 最小宽 */
  defaults: Record<C, number>;
  mins: Record<C, number>;
  /** 弹性列最小宽保底（默认 140） */
  elasticMin?: number;
  /** 行内除可调固定列与弹性列外的其余固定占宽合计（左右 padding、勾选框列等），默认 28 */
  padTotal?: number;
}

export function useColResize<C extends string>(
  spec: ColResizeSpec<C>,
  rootEl: Ref<HTMLElement | null>,
) {
  let stored = widthStore.get(spec.id);
  if (!stored) {
    stored = reactive<Record<string, number>>({ ...spec.defaults });
    widthStore.set(spec.id, stored);
  }
  const w = stored as Record<C, number>;

  /** 列宽经 CSS 变量 --col-<列名> 下发，表头与数据行共用列类名天然对齐 */
  const colVars = computed(() => {
    const vars = {} as Record<`--col-${C}`, string>;
    for (const c of spec.cols) {
      vars[`--col-${c}`] = `${w[c]}px`;
    }
    return vars;
  });

  /** 除 key 外其余可调固定列的宽度合计 */
  function othersOf(key: C): number {
    let sum = 0;
    for (const c of spec.cols) {
      if (c !== key) sum += w[c];
    }
    return sum;
  }

  /**
   * 拖拽 left|right 两列之间的分界线；left 传 null 表示弹性列（宽度随右列反向自动变化）。
   * 分界线位移 = 左列加宽量 = 右列缩窄量，只动相邻两列。
   */
  function startResize(left: C | null, right: C, e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startLeft = left ? w[left] : 0;
    const startRight = w[right];
    // 右移上限：右列缩到最小宽；左移下限：左列缩到最小宽（弹性列按下限宽换算成右列的最大宽）
    const shiftMax = startRight - spec.mins[right];
    const shiftMin = left
      ? spec.mins[left] - startLeft
      : startRight -
        (rootEl.value
          ? rootEl.value.clientWidth -
            (spec.padTotal ?? 28) -
            (spec.elasticMin ?? 140) -
            othersOf(right)
          : startRight);
    const onMove = (ev: MouseEvent) => {
      const shift = Math.min(shiftMax, Math.max(shiftMin, ev.clientX - startX));
      if (left) {
        w[left] = startLeft + shift;
      }
      w[right] = startRight - shift;
    };
    const onUp = () => {
      document.body.classList.remove("col-resizing");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    document.body.classList.add("col-resizing");
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  /** 双击分界线：复位默认列宽 */
  function resetWidths() {
    for (const c of spec.cols) {
      w[c] = spec.defaults[c];
    }
  }

  return { colVars, startResize, resetWidths };
}
