import { useMemo } from 'react';
import type { Microphone } from '../../common/capture';
import { Select, type Option } from './Select';

/**
 * Microphone picker. `audio.input_device` stores the microphone label (or
 * `"default"`), so a saved device that is unplugged is still listed.
 */
export function MicSelect({
  value,
  mics,
  onChange,
  width = 280,
  ...aria
}: {
  value: string;
  mics: Microphone[];
  onChange: (device: string) => void;
  width?: number | string;
  'aria-label'?: string;
}) {
  const options = useMemo(() => {
    const list: Option[] = [{ value: 'default', label: 'System default' }];
    const seen = new Set<string>();
    for (const mic of mics) {
      if (seen.has(mic.label)) continue;
      seen.add(mic.label);
      list.push({ value: mic.label, label: mic.label });
    }
    if (value !== 'default' && !seen.has(value)) {
      list.push({ value, label: value, hint: 'Not connected' });
    }
    return list;
  }, [mics, value]);
  return (
    <Select
      value={value || 'default'}
      options={options}
      onChange={onChange}
      width={width}
      {...aria}
    />
  );
}
