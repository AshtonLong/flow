import { DEFAULT_CONFIG, HOTKEY_ACTIONS, type HotkeyAction } from '@shared/config';
import { findConflicts } from '@shared/hotkeys';
import { Group, PageHeader, SettingRow } from '../components/Card';
import { HotkeyRecorder } from '../components/HotkeyRecorder';
import { Toggle } from '../components/Toggle';
import { useConfig } from '../lib/config';

export const ACTIONS: Record<HotkeyAction, { label: string; description: string }> = {
  hold_to_talk: {
    label: 'Hold to talk',
    description: 'Records while held. Release to type what you said.',
  },
  toggle: {
    label: 'Toggle',
    description: 'Press once to start recording and again to stop.',
  },
  cancel: {
    label: 'Cancel',
    description: 'While recording or transcribing, discards it and types nothing.',
  },
  paste_last: {
    label: 'Paste last',
    description: 'Inserts the previous transcript again.',
  },
};

export function Hotkeys() {
  const { config, set } = useConfig();
  const bindings = Object.fromEntries(
    HOTKEY_ACTIONS.map((action) => [action, config.hotkeys[action]]),
  );
  const conflicts = findConflicts(bindings);

  const problemFor = (action: HotkeyAction): string | null => {
    const mine = conflicts.filter((c) => c.action === action);
    if (mine.some((c) => c.kind === 'invalid')) {
      return `“${config.hotkeys[action]}” is not a key Flow recognises. Record a new one.`;
    }
    const same = mine
      .filter((c) => c.kind === 'same')
      .map((c) => ACTIONS[c.other as HotkeyAction]?.label ?? c.other);
    if (same.length > 0) {
      return `Same keys as ${same.join(' and ')}. Only one of them can respond, so change one.`;
    }
    return null;
  };

  return (
    <>
      <PageHeader
        title="Hotkeys"
        lead="Any key, combination, lone modifier (such as Right Ctrl) or mouse button works. Hotkeys work in every app, including full-screen ones."
      />

      <Group title="Actions">
        {HOTKEY_ACTIONS.map((action) => {
          const problem = problemFor(action);
          return (
            <SettingRow
              key={action}
              label={ACTIONS[action].label}
              description={ACTIONS[action].description}
              control={
                <HotkeyRecorder
                  actionName={ACTIONS[action].label}
                  value={config.hotkeys[action]}
                  defaultValue={DEFAULT_CONFIG.hotkeys[action]}
                  onChange={(binding) => set(['hotkeys', action], binding)}
                />
              }
            >
              {problem && (
                <p role="alert" className="mt-2 text-caption text-danger">
                  {problem}
                </p>
              )}
            </SettingRow>
          );
        })}
      </Group>

      <Group title="Hands-free">
        <SettingRow
          label="Double-tap to lock"
          description="Double-tap the hold-to-talk hotkey to keep recording until you tap it again."
          control={
            <Toggle
              checked={config.hotkeys.double_tap_lock}
              onChange={(v) => set(['hotkeys', 'double_tap_lock'], v)}
            />
          }
        />
      </Group>
    </>
  );
}
