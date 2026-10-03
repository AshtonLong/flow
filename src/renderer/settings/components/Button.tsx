import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { Tooltip } from 'radix-ui';

type Variant = 'standard' | 'accent' | 'subtle';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: 'md' | 'sm';
  danger?: boolean;
  icon?: ReactNode;
}

function classes(variant: Variant, size: 'md' | 'sm', danger: boolean, extra?: string): string {
  return [
    'btn',
    variant === 'accent' && 'btn-accent',
    variant === 'subtle' && 'btn-subtle',
    size === 'sm' && 'btn-sm',
    danger && 'btn-danger',
    extra,
  ]
    .filter(Boolean)
    .join(' ');
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'standard', size = 'md', danger = false, icon, className, children, ...rest },
  ref,
) {
  return (
    <button ref={ref} type="button" className={classes(variant, size, danger, className)} {...rest}>
      {icon && (
        <span aria-hidden="true" className="-ml-0.5 flex">
          {icon}
        </span>
      )}
      {children}
    </button>
  );
});

interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children'> {
  /** The accessible name, also shown as a tooltip. */
  label: string;
  icon: ReactNode;
  size?: 'md' | 'sm';
  danger?: boolean;
  variant?: Variant;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon, size = 'md', danger = false, variant = 'subtle', className, ...rest },
  ref,
) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>
        <button
          ref={ref}
          type="button"
          aria-label={label}
          className={classes(variant, size, danger, `btn-icon ${className ?? ''}`)}
          {...rest}
        >
          <span aria-hidden="true" className="flex">
            {icon}
          </span>
        </button>
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="tooltip" sideOffset={6}>
          {label}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
});

/** A text link that opens an https page in the default browser. */
export function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <button type="button" className="link" onClick={() => void window.flow.app.openExternal(href)}>
      {children}
    </button>
  );
}
