/** Page scaffolding: a titled group of setting cards, and the label/explanation/control row. */
import { createContext, useContext, useId, type ReactNode } from 'react';

interface FieldIds {
  labelId: string;
  descriptionId?: string;
}

const FieldContext = createContext<FieldIds | null>(null);

/**
 * Accessible-name props for a control. Inside a `SettingRow` the control is
 * labelled by the row's label and described by its explanation, unless the
 * control passes its own `aria-label`.
 */
export function useFieldProps(own: {
  'aria-label'?: string;
  'aria-labelledby'?: string;
  'aria-describedby'?: string;
}): { 'aria-label'?: string; 'aria-labelledby'?: string; 'aria-describedby'?: string } {
  const field = useContext(FieldContext);
  if (own['aria-label'] || own['aria-labelledby']) {
    return {
      'aria-label': own['aria-label'],
      'aria-labelledby': own['aria-labelledby'],
      'aria-describedby': own['aria-describedby'],
    };
  }
  return {
    'aria-labelledby': field?.labelId,
    'aria-describedby': own['aria-describedby'] ?? field?.descriptionId,
  };
}

export function PageHeader({ title, lead }: { title: string; lead?: ReactNode }) {
  return (
    <header className="mb-6">
      <h1 className="font-display text-title font-[840]">{title}</h1>
      {lead && <p className="mt-1 max-w-[72ch] text-fg-2">{lead}</p>}
    </header>
  );
}

export function Group({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description?: ReactNode;
  /** A button aligned with the heading, e.g. "Add word". */
  action?: ReactNode;
  children: ReactNode;
}) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="mb-8">
      <div className="mb-2 flex min-h-8 items-end justify-between gap-4">
        <div>
          <h2 id={headingId} className="font-semibold">
            {title}
          </h2>
          {description && (
            <p className="mt-0.5 max-w-[78ch] text-caption text-fg-2">{description}</p>
          )}
        </div>
        {action}
      </div>
      <div className="flex flex-col gap-[3px]">{children}</div>
    </section>
  );
}

export function Card({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`card ${className}`}>{children}</div>;
}

/**
 * One setting: label, a one-line explanation beneath it, and the control on
 * the right. `children` render full-width under the row (meters, errors).
 */
export function SettingRow({
  label,
  description,
  control,
  children,
  disabled = false,
}: {
  label: ReactNode;
  description: ReactNode;
  control?: ReactNode;
  children?: ReactNode;
  disabled?: boolean;
}) {
  const labelId = useId();
  const descriptionId = useId();
  return (
    <FieldContext.Provider value={{ labelId, descriptionId }}>
      <div className="card px-4 py-3">
        <div className="flex min-h-10 items-center gap-6">
          <div className="min-w-0 flex-1">
            <div id={labelId} className={disabled ? 'text-fg-3' : undefined}>
              {label}
            </div>
            <div id={descriptionId} className="mt-0.5 text-caption text-fg-2">
              {description}
            </div>
          </div>
          {control && <div className="flex shrink-0 items-center gap-2">{control}</div>}
        </div>
        {children}
      </div>
    </FieldContext.Provider>
  );
}

/** One sentence and one action. */
export function EmptyState({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="card flex items-center justify-between gap-6 px-4 py-4">
      <p className="text-fg-2">{children}</p>
      {action}
    </div>
  );
}
