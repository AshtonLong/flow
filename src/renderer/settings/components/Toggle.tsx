import { Switch } from 'radix-ui';
import { useFieldProps } from './Card';

/** A Windows-style toggle with its On/Off state written beside it. */
export function Toggle({
  checked,
  onChange,
  disabled,
  hideState = false,
  ...aria
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Drops the On/Off text where space is tight. */
  hideState?: boolean;
  'aria-label'?: string;
  'aria-labelledby'?: string;
  'aria-describedby'?: string;
}) {
  const field = useFieldProps(aria);
  return (
    <span className="flex items-center gap-3">
      {!hideState && (
        <span aria-hidden="true" className={disabled ? 'text-fg-3' : undefined}>
          {checked ? 'On' : 'Off'}
        </span>
      )}
      <Switch.Root
        className="switch"
        checked={checked}
        onCheckedChange={onChange}
        disabled={disabled}
        {...field}
      >
        <Switch.Thumb className="switch-thumb" />
      </Switch.Root>
    </span>
  );
}
