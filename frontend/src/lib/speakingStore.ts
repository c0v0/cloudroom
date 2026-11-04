/** 说话电平：模块级 store，只有订阅的组件（成员头像）会重渲染。 */
import { useSyncExternalStore } from "react";

type Levels = Record<string, number>;

let levels: Levels = {};
const listeners = new Set<() => void>();

export function setSpeakingLevels(next: Levels): void {
  levels = next;
  for (const fn of listeners) fn();
}

function getSnapshot(): Levels {
  return levels;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** 仅在需要显示说话动效的叶子组件里调用。 */
export function useSpeakingLevels(): Levels {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
