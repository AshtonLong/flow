/**
 * Editing state for list settings (dictionary, snippets). Rows being typed can
 * be incomplete; only complete rows are written to the file, and the list
 * resets when the file changes underneath.
 */
import { useCallback, useEffect, useRef, useState } from 'react';

export interface DraftRow<T> {
  key: number;
  value: T;
}

export interface ListDraft<T> {
  rows: DraftRow<T>[];
  add(value: T): void;
  update(key: number, value: T): void;
  remove(key: number): void;
}

export function useListDraft<T>(
  saved: T[],
  isComplete: (row: T) => boolean,
  commit: (rows: T[]) => void,
): ListDraft<T> {
  const nextKey = useRef(0);
  const wrap = (values: T[]): DraftRow<T>[] =>
    values.map((value) => ({ key: nextKey.current++, value }));
  const signature = (values: T[]) => JSON.stringify(values);

  const [rows, setRows] = useState<DraftRow<T>[]>(() => wrap(saved));
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const committed = useRef(signature(saved));

  useEffect(() => {
    const incoming = signature(saved);
    if (incoming === committed.current) return;
    committed.current = incoming;
    const local = rowsRef.current.filter((r) => isComplete(r.value)).map((r) => r.value);
    if (signature(local) !== incoming) setRows(wrap(saved));
    // `saved` is the only trigger: this reacts to the file changing.
  }, [saved]);

  const apply = useCallback(
    (next: DraftRow<T>[]) => {
      rowsRef.current = next;
      setRows(next);
      const complete = next.filter((r) => isComplete(r.value)).map((r) => r.value);
      const sig = signature(complete);
      if (sig !== committed.current) {
        committed.current = sig;
        commit(complete);
      }
    },
    [commit],
  );

  return {
    rows,
    add: (value) => apply([...rowsRef.current, { key: nextKey.current++, value }]),
    update: (key, value) => apply(rowsRef.current.map((r) => (r.key === key ? { key, value } : r))),
    remove: (key) => apply(rowsRef.current.filter((r) => r.key !== key)),
  };
}
