import { useRef, useState, type KeyboardEvent } from 'react';
import { X } from 'lucide-react';

/**
 * A list of short strings edited as chips. Enter or a comma adds the typed
 * text; Backspace in an empty box removes the last chip.
 */
export function ChipsInput({
  values,
  onChange,
  placeholder,
  label,
  mono = false,
}: {
  values: string[];
  onChange: (values: string[]) => void;
  placeholder?: string;
  /** Accessible name of the text box, e.g. "Heard phrases for Tauri". */
  label: string;
  mono?: boolean;
}) {
  const [text, setText] = useState('');
  const input = useRef<HTMLInputElement>(null);

  const add = (raw: string) => {
    const parts = raw
      .split(',')
      .map((p) => p.trim())
      .filter(Boolean);
    setText('');
    if (parts.length === 0) return;
    const next = [...values];
    for (const part of parts) {
      if (!next.some((v) => v.toLowerCase() === part.toLowerCase())) next.push(part);
    }
    if (next.length !== values.length) onChange(next);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      add(text);
    } else if (e.key === 'Backspace' && text === '' && values.length > 0) {
      onChange(values.slice(0, -1));
    }
  };

  return (
    <div
      className="input flex h-auto min-h-8 flex-wrap items-center gap-1 px-1.5 py-[3px]"
      onClick={(e) => {
        if (e.target === e.currentTarget) input.current?.focus();
      }}
    >
      {values.map((value) => (
        <span
          key={value}
          className={`inline-flex h-6 items-center gap-0.5 rounded-sm border border-line bg-hover pr-0.5 pl-2 text-caption ${mono ? 'font-mono' : ''}`}
        >
          {value}
          <button
            type="button"
            aria-label={`Remove ${value}`}
            className="flex size-5 items-center justify-center rounded-sm text-fg-2 hover:bg-hover hover:text-fg"
            onClick={() => onChange(values.filter((v) => v !== value))}
          >
            <X size={12} aria-hidden="true" />
          </button>
        </span>
      ))}
      <input
        ref={input}
        type="text"
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        placeholder={values.length === 0 ? placeholder : undefined}
        className={`h-6 min-w-[72px] flex-1 bg-transparent px-1 outline-none placeholder:text-fg-3 ${mono ? 'font-mono text-[13px]' : ''}`}
        value={text}
        onChange={(e) => {
          if (e.target.value.includes(',')) add(e.target.value);
          else setText(e.target.value);
        }}
        onKeyDown={onKeyDown}
        onBlur={() => add(text)}
      />
    </div>
  );
}
