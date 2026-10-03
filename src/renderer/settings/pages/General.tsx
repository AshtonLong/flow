import { OVERLAY_POSITIONS, type Config } from '@shared/config';
import { Group, PageHeader, SettingRow } from '../components/Card';
import { MicMeter } from '../components/LevelMeter';
import { MicSelect } from '../components/MicSelect';
import { Select, type Option } from '../components/Select';
import { NumberField } from '../components/TextField';
import { Toggle } from '../components/Toggle';
import { useConfig } from '../lib/config';
import { formatDuration } from '../lib/format';
import { useMicrophones } from '../lib/hooks';

const THEMES: Option<Config['general']['theme']>[] = [
  { value: 'system', label: 'Use Windows setting' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
];

const STYLES: Option<Config['overlay']['style']>[] = [
  { value: 'pill', label: 'Pill' },
  { value: 'minimal', label: 'Minimal' },
  { value: 'none', label: 'Hidden' },
];

const POSITION_LABELS: Record<(typeof OVERLAY_POSITIONS)[number], string> = {
  'bottom-center': 'Bottom centre',
  'bottom-left': 'Bottom left',
  'bottom-right': 'Bottom right',
  'top-center': 'Top centre',
  'top-left': 'Top left',
  'top-right': 'Top right',
};

const POSITIONS = OVERLAY_POSITIONS.map((value) => ({ value, label: POSITION_LABELS[value] }));

const SIZES: Option<Config['overlay']['size']>[] = [
  { value: 'small', label: 'Small' },
  { value: 'medium', label: 'Medium' },
  { value: 'large', label: 'Large' },
];

export function General() {
  const { config, set } = useConfig();
  const { mics, refresh } = useMicrophones();
  const overlayOff = config.overlay.style === 'none';

  return (
    <>
      <PageHeader title="General" />

      <Group title="App">
        <SettingRow
          label="Launch at login"
          description="Start Flow in the system tray when you sign in to Windows."
          control={
            <Toggle
              checked={config.general.launch_at_login}
              onChange={(v) => set(['general', 'launch_at_login'], v)}
            />
          }
        />
        <SettingRow
          label="Theme"
          description="Light, dark, or whatever Windows is set to."
          control={
            <Select
              value={config.general.theme}
              options={THEMES}
              onChange={(v) => set(['general', 'theme'], v)}
            />
          }
        />
        <SettingRow
          label="Language"
          description="Flow understands English for now. More languages come in a later version."
          control={
            <Select
              value="en"
              options={[{ value: 'en', label: 'English' }]}
              onChange={() => {}}
              disabled
            />
          }
        />
      </Group>

      <Group title="Microphone">
        <SettingRow
          label="Microphone"
          description="The device Flow listens to. Speak to check the level below."
          control={
            <MicSelect
              value={config.audio.input_device}
              mics={mics}
              onChange={(v) => set(['audio', 'input_device'], v)}
            />
          }
        >
          <div className="mt-3">
            <MicMeter device={config.audio.input_device} onListening={refresh} />
          </div>
        </SettingRow>
        <SettingRow
          label="Keep microphone warm"
          description="Recording starts faster, but the Windows microphone indicator stays on all the time."
          control={
            <Toggle
              checked={config.audio.keep_mic_warm}
              onChange={(v) => set(['audio', 'keep_mic_warm'], v)}
            />
          }
        />
        <SettingRow
          label="Sounds"
          description="Play a short sound when recording starts and stops."
          control={
            <Toggle checked={config.audio.sounds} onChange={(v) => set(['audio', 'sounds'], v)} />
          }
        />
        <SettingRow
          label="Maximum recording length"
          description={`A recording stops by itself after ${formatDuration(config.audio.max_recording_seconds * 1000)}, in case a key gets stuck.`}
          control={
            <NumberField
              value={config.audio.max_recording_seconds}
              min={5}
              max={3600}
              unit="seconds"
              onCommit={(v) => set(['audio', 'max_recording_seconds'], v)}
            />
          }
        />
      </Group>

      <Group title="Overlay" description="The small indicator shown on screen while you dictate.">
        <SettingRow
          label="Style"
          description="Pill shows status and waveform, Minimal is a small dot, Hidden shows nothing."
          control={
            <Select
              value={config.overlay.style}
              options={STYLES}
              onChange={(v) => set(['overlay', 'style'], v)}
            />
          }
        />
        <SettingRow
          label="Position"
          description="Where on the screen the overlay appears."
          disabled={overlayOff}
          control={
            <Select
              value={config.overlay.position}
              options={POSITIONS}
              disabled={overlayOff}
              onChange={(v) => set(['overlay', 'position'], v)}
            />
          }
        />
        <SettingRow
          label="Size"
          description="How large the overlay is drawn."
          disabled={overlayOff}
          control={
            <Select
              value={config.overlay.size}
              options={SIZES}
              disabled={overlayOff}
              onChange={(v) => set(['overlay', 'size'], v)}
            />
          }
        />
        <SettingRow
          label="Show waveform"
          description="Draw your voice as a live waveform while recording."
          disabled={overlayOff}
          control={
            <Toggle
              checked={config.overlay.show_waveform}
              disabled={overlayOff}
              onChange={(v) => set(['overlay', 'show_waveform'], v)}
            />
          }
        />
      </Group>
    </>
  );
}
