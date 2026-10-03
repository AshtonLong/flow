/** Applies config patches to a plain object, for optimistic updates and the mock backend. */
import type { ConfigPatch, ConfigPath } from '@shared/config';

type Node = Record<string | number, unknown>;

function isContainer(value: unknown): value is Node {
  return value !== null && typeof value === 'object';
}

/** Returns a patched deep copy. `value: undefined` removes the key (or splices an array item). */
export function applyPatches<T>(root: T, patches: readonly ConfigPatch[]): T {
  const next = structuredClone(root);
  for (const { path, value } of patches) {
    if (path.length === 0) continue;
    let node: Node | null = next as unknown as Node;
    for (let i = 0; i < path.length - 1; i++) {
      const key = path[i]!;
      let child: unknown = node[key];
      if (!isContainer(child)) {
        if (value === undefined) {
          node = null;
          break;
        }
        child = typeof path[i + 1] === 'number' ? [] : {};
        node[key] = child;
      }
      node = child as Node;
    }
    if (!node) continue;
    const last = path[path.length - 1]!;
    if (value === undefined) {
      if (Array.isArray(node) && typeof last === 'number') node.splice(last, 1);
      else delete node[last];
    } else {
      node[last] = structuredClone(value);
    }
  }
  return next;
}

/** `["audio", "max_recording_seconds"]` → `audio.max_recording_seconds`; indexes as `[n]`. */
export function formatPath(path: ConfigPath): string {
  return path.reduce<string>((out, part) => {
    if (typeof part === 'number') return `${out}[${part}]`;
    return out ? `${out}.${part}` : part;
  }, '');
}
