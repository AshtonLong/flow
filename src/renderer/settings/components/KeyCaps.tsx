import { displayTokens } from '@shared/hotkeys';

/** A binding drawn as key caps: `Ctrl+Win` → [Ctrl] [Win]. */
export function KeyCaps({ binding, muted = false }: { binding: string; muted?: boolean }) {
  const tokens = displayTokens(binding);
  if (tokens.length === 0) return <span className="text-fg-3">Not set</span>;
  return (
    <span className={`inline-flex items-center gap-1 ${muted ? 'opacity-60' : ''}`}>
      {/* Screen readers get the binding as one phrase rather than separate boxes. */}
      <span className="sr-only">{tokens.join(' plus ')}</span>
      {tokens.map((token, i) => (
        <kbd key={`${token}-${i}`} aria-hidden="true" className="keycap font-sans">
          {token}
        </kbd>
      ))}
    </span>
  );
}
