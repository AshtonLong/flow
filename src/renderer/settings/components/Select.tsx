import type { ReactNode } from 'react';
import { Select as RadixSelect } from 'radix-ui';
import { ChevronDown } from 'lucide-react';
import { useFieldProps } from './Card';

export interface Option<T extends string = string> {
  value: T;
  label: string;
  /** Shown after the label in the list, e.g. a price or "Recommended". */
  hint?: ReactNode;
  disabled?: boolean;
}

/** A Windows-style combo box. Option values must be non-empty strings. */
export function Select<T extends string>({
  value,
  options,
  onChange,
  disabled,
  width = 200,
  placeholder,
  ...aria
}: {
  value: T | undefined;
  options: readonly Option<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** Trigger width in px, or a CSS width. */
  width?: number | string;
  placeholder?: string;
  'aria-label'?: string;
  'aria-labelledby'?: string;
  'aria-describedby'?: string;
}) {
  const field = useFieldProps(aria);
  return (
    <RadixSelect.Root
      value={value}
      onValueChange={(next) => onChange(next as T)}
      disabled={disabled}
    >
      <RadixSelect.Trigger
        className="btn justify-between gap-3 text-left"
        style={{ width }}
        {...field}
      >
        <span className="min-w-0 truncate">
          <RadixSelect.Value placeholder={placeholder} />
        </span>
        <RadixSelect.Icon className="flex shrink-0 text-fg-2">
          <ChevronDown size={14} aria-hidden="true" />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content
          className="flyout max-h-(--radix-select-content-available-height) min-w-(--radix-select-trigger-width) overflow-hidden"
          position="popper"
          sideOffset={4}
          collisionPadding={12}
        >
          <RadixSelect.Viewport>
            {options.map((option) => (
              <RadixSelect.Item
                key={option.value}
                value={option.value}
                disabled={option.disabled}
                className="flyout-item"
              >
                <RadixSelect.ItemText>{option.label}</RadixSelect.ItemText>
                {option.hint && (
                  <span className="ml-auto pl-4 text-caption text-fg-2">{option.hint}</span>
                )}
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}
