import type { ReactNode } from 'react';
import { CircleAlert, CircleCheck, Info, TriangleAlert } from 'lucide-react';

type Tone = 'info' | 'ok' | 'warn' | 'danger';

const ICONS = { info: Info, ok: CircleCheck, warn: TriangleAlert, danger: CircleAlert } as const;

const TONES: Record<Tone, string> = {
  info: 'bg-inset text-fg-2',
  ok: 'bg-ok-soft text-fg',
  warn: 'bg-warn-soft text-fg',
  danger: 'bg-danger-soft text-fg',
};

const ICON_TONES: Record<Tone, string> = {
  info: 'text-fg-2',
  ok: 'text-ok',
  warn: 'text-warn',
  danger: 'text-danger',
};

/** An inline message with one optional action. Errors are announced at once. */
export function Notice({
  tone = 'info',
  children,
  action,
  className = '',
}: {
  tone?: Tone;
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  const Icon = ICONS[tone];
  return (
    <div
      role={tone === 'danger' ? 'alert' : 'status'}
      className={`flex items-start gap-2.5 rounded border border-line px-3 py-2 text-caption ${TONES[tone]} ${className}`}
    >
      <Icon size={16} aria-hidden="true" className={`shrink-0 ${ICON_TONES[tone]}`} />
      <div className="selectable min-w-0 flex-1">{children}</div>
      {action && <div className="-my-0.5 shrink-0">{action}</div>}
    </div>
  );
}

export function Badge({
  tone = 'neutral',
  children,
}: {
  tone?: 'neutral' | 'accent' | 'ok' | 'warn';
  children: ReactNode;
}) {
  const cls =
    tone === 'accent'
      ? 'badge-accent'
      : tone === 'ok'
        ? 'badge-ok'
        : tone === 'warn'
          ? 'badge-warn'
          : '';
  return <span className={`badge ${cls}`}>{children}</span>;
}

/** Determinate when `value` (0..1) is given, otherwise a looping bar. */
export function ProgressBar({ value, label }: { value?: number; label: string }) {
  const percent =
    value === undefined ? undefined : Math.round(Math.min(1, Math.max(0, value)) * 100);
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="meter-track h-1"
    >
      {percent === undefined ? (
        <div
          className="h-full w-2/5 rounded-full bg-accent"
          style={{ animation: 'flow-indeterminate 1.4s ease-in-out infinite' }}
        />
      ) : (
        <div
          className="h-full rounded-full bg-accent transition-[width] duration-200"
          style={{ width: `${percent}%` }}
        />
      )}
    </div>
  );
}
