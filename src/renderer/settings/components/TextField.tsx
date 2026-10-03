/**
 * Text inputs that hold a local draft and commit on blur or Enter (and, for
 * free text, after a pause in typing). While focused, a draft is never
 * overwritten by a config reload.
 */
import {
  useEffect,
  useRef,
  useState,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type TextareaHTMLAttributes,
} from 'react';
import { useFieldProps } from './Card';

const DEBOUNCE_MS = 700;

function useDraft(
  value: string,
  onCommit: (value: string) => void,
  debounce: boolean,
): {
  draft: string;
  setDraft: (next: string) => void;
  flush: () => void;
  revert: () => void;
  onFocus: () => void;
  onBlur: () => void;
} {
  const [draft, setDraftState] = useState(value);
  const focused = useRef(false);
  const latest = useRef(draft);
  const committed = useRef(value);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;

  useEffect(() => {
    committed.current = value;
    if (!focused.current) {
      latest.current = value;
      setDraftState(value);
    }
  }, [value]);

  const clear = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };
  const flush = () => {
    clear();
    if (latest.current !== committed.current) {
      committed.current = latest.current;
      commitRef.current(latest.current);
    }
  };
  // A pending edit is still saved if the field unmounts (page change, dialog close).
  useEffect(() => flush, []);

  return {
    draft,
    setDraft(next) {
      latest.current = next;
      setDraftState(next);
      if (debounce) {
        clear();
        timer.current = setTimeout(flush, DEBOUNCE_MS);
      }
    },
    flush,
    revert() {
      clear();
      latest.current = committed.current;
      setDraftState(committed.current);
    },
    onFocus() {
      focused.current = true;
    },
    onBlur() {
      focused.current = false;
      flush();
    },
  };
}

type AriaNames = {
  'aria-label'?: string;
  'aria-labelledby'?: string;
  'aria-describedby'?: string;
};

interface TextFieldProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, 'value' | 'onChange' | 'width'>, AriaNames {
  value: string;
  onCommit: (value: string) => void;
  width?: number | string;
  /** Returns an error message for a draft that must not be saved. */
  validate?: (value: string) => string | null;
  mono?: boolean;
}

export function TextField({
  value,
  onCommit,
  width = 200,
  validate,
  mono,
  className = '',
  ...rest
}: TextFieldProps) {
  const [error, setError] = useState<string | null>(null);
  const draft = useDraft(
    value,
    (next) => {
      const problem = validate?.(next) ?? null;
      setError(problem);
      if (!problem) onCommit(next);
    },
    true,
  );
  const field = useFieldProps(rest);
  return (
    <span className="inline-flex flex-col gap-1" style={{ width }}>
      <input
        type="text"
        spellCheck={false}
        autoComplete="off"
        {...rest}
        {...field}
        className={`input w-full ${mono ? 'font-mono text-[13px]' : ''} ${className}`}
        value={draft.draft}
        aria-invalid={error ? true : undefined}
        onChange={(e) => {
          setError(null);
          draft.setDraft(e.target.value);
        }}
        onFocus={draft.onFocus}
        onBlur={draft.onBlur}
        onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
          if (e.key === 'Enter') draft.flush();
          else if (e.key === 'Escape' && draft.draft !== value) {
            e.stopPropagation();
            setError(null);
            draft.revert();
          }
        }}
      />
      {error && (
        <span role="alert" className="text-caption text-danger">
          {error}
        </span>
      )}
    </span>
  );
}

interface NumberFieldProps extends AriaNames {
  value: number;
  onCommit: (value: number) => void;
  min: number;
  max: number;
  /** Unit shown after the box: "seconds", "ms", "days". */
  unit?: string;
  disabled?: boolean;
}

/** Whole numbers, clamped to the range the config accepts. Commits on blur or Enter. */
export function NumberField({
  value,
  onCommit,
  min,
  max,
  unit,
  disabled,
  ...aria
}: NumberFieldProps) {
  const draft = useDraft(
    String(value),
    (text) => {
      const parsed = Number.parseInt(text, 10);
      if (Number.isNaN(parsed)) return;
      const clamped = Math.min(max, Math.max(min, parsed));
      if (clamped !== value) onCommit(clamped);
    },
    false,
  );
  const field = useFieldProps(aria);
  // Shows what was actually saved: the clamped number, or the old value for junk.
  const settle = () => {
    draft.flush();
    const parsed = Number.parseInt(draft.draft, 10);
    const shown = Number.isNaN(parsed) ? value : Math.min(max, Math.max(min, parsed));
    if (String(shown) !== draft.draft) draft.setDraft(String(shown));
  };
  return (
    <span className="flex items-center gap-2">
      <input
        type="text"
        inputMode="numeric"
        autoComplete="off"
        disabled={disabled}
        {...field}
        className="input w-[88px] text-right tabular-nums"
        value={draft.draft}
        onChange={(e) => draft.setDraft(e.target.value.replace(/[^\d-]/g, ''))}
        onFocus={draft.onFocus}
        onBlur={() => {
          settle();
          draft.onBlur();
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') settle();
          else if (e.key === 'Escape') draft.revert();
        }}
      />
      {unit && (
        <span aria-hidden="true" className="text-fg-2">
          {unit}
        </span>
      )}
    </span>
  );
}

interface TextAreaProps
  extends Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, 'value' | 'onChange'>, AriaNames {
  value: string;
  onCommit: (value: string) => void;
}

export function TextArea({ value, onCommit, className = '', rows = 3, ...rest }: TextAreaProps) {
  const draft = useDraft(value, onCommit, true);
  const field = useFieldProps(rest);
  return (
    <textarea
      spellCheck={false}
      rows={rows}
      {...rest}
      {...field}
      className={`input w-full ${className}`}
      value={draft.draft}
      onChange={(e) => draft.setDraft(e.target.value)}
      onFocus={draft.onFocus}
      onBlur={draft.onBlur}
    />
  );
}
