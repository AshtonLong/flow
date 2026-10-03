import { useState } from 'react';
import type { Config } from '@shared/config';
import { Button, ExternalLink } from '../components/Button';
import { Group, PageHeader, SettingRow } from '../components/Card';
import { Notice } from '../components/Notice';
import { Select, type Option } from '../components/Select';
import { Toggle } from '../components/Toggle';
import { useCatalog } from '../lib/catalog';
import { useConfig } from '../lib/config';
import { errorMessage } from '../lib/format';
import { useAppInfo } from '../lib/hooks';

const METHODS: Option<Config['insert']['method']>[] = [
  { value: 'paste', label: 'Paste' },
  { value: 'type', label: 'Type' },
];

const SHORTCUTS = ['Ctrl+V', 'Ctrl+Shift+V', 'Shift+Insert'];

function PathLine({ path }: { path: string | undefined }) {
  if (!path) return null;
  return (
    <span className="selectable mt-1 block font-mono text-[11px] break-all text-fg-2">{path}</span>
  );
}

function About() {
  const { info } = useAppInfo();
  const { snapshot } = useCatalog();
  const models = snapshot ? [...snapshot.catalog.local] : [];
  const needsAttribution = (licence: string) => /^cc-by/i.test(licence);

  return (
    <Group title="About">
      <div className="card px-4 py-3">
        <div className="flex items-baseline gap-3">
          <span className="font-display text-subtitle font-[760]">Flow</span>
          <span className="selectable text-fg-2">
            {info ? `Version ${info.version}` : 'Version unknown'}
          </span>
        </div>
        <p className="mt-1 text-caption text-fg-2">
          Free and open source under the MIT licence. No account, no telemetry.
          {info && ` Built on Electron ${info.electron}.`}
        </p>
      </div>

      {info && info.degraded.length > 0 && (
        <div className="card px-4 py-3">
          <div>Unavailable features</div>
          <p className="mt-0.5 text-caption text-fg-2">
            Parts of Flow that could not start on this PC. Reinstalling usually fixes them.
          </p>
          <ul className="mt-2 flex flex-col gap-1.5">
            {info.degraded.map((item) => (
              <li key={item.module}>
                <Notice tone="warn">
                  {item.effect} <span className="font-mono text-fg-2">({item.module})</span>
                </Notice>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="card px-4 py-3">
        <div>Model licences</div>
        <p className="mt-0.5 text-caption text-fg-2">
          Each speech model is published by its authors under its own licence.
        </p>
        {models.length === 0 ? (
          <p className="mt-2 text-caption text-fg-2">The catalog has not loaded yet.</p>
        ) : (
          <table className="mt-2 w-full border-collapse text-left text-caption">
            <thead className="sr-only">
              <tr>
                <th scope="col">Model</th>
                <th scope="col">Licence</th>
              </tr>
            </thead>
            <tbody>
              {models.map((m) => (
                <tr key={m.id} className="border-t border-line">
                  <th scope="row" className="py-1.5 pr-4 font-normal text-fg">
                    {m.name}
                  </th>
                  <td className="py-1.5 text-right whitespace-nowrap">
                    {needsAttribution(m.licence) && (
                      <span className="mr-2 text-fg-2">Attribution required</span>
                    )}
                    {m.licenceUrl?.startsWith('https://') ? (
                      <ExternalLink href={m.licenceUrl}>{m.licence}</ExternalLink>
                    ) : (
                      m.licence
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {models.some((m) => needsAttribution(m.licence)) && (
          <p className="selectable mt-2 border-t border-line pt-2 text-caption text-fg-2">
            {models
              .filter((m) => needsAttribution(m.licence))
              .map((m) => `${m.name} is used under ${m.licence}.`)
              .join(' ')}{' '}
            Local models run on transcribe.cpp, MIT licence.
          </p>
        )}
      </div>
    </Group>
  );
}

export function Advanced() {
  const { config, set, snapshot } = useConfig();
  const { info } = useAppInfo();
  const [transfer, setTransfer] = useState<{ ok: boolean; message: string } | null>(null);
  const [update, setUpdate] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const typing = config.insert.method === 'type';

  const shortcutOptions: Option[] = SHORTCUTS.map((s) => ({ value: s, label: s }));
  if (!SHORTCUTS.includes(config.insert.paste_shortcut)) {
    shortcutOptions.push({
      value: config.insert.paste_shortcut,
      label: config.insert.paste_shortcut,
      hint: 'From the file',
    });
  }

  const runTransfer = async (kind: 'export' | 'import') => {
    try {
      const done =
        kind === 'export'
          ? await window.flow.config.exportFile()
          : await window.flow.config.importFile();
      setTransfer(
        done
          ? {
              ok: true,
              message:
                kind === 'export'
                  ? 'Settings exported. API keys were left out.'
                  : 'Settings imported. Reconnect your API keys on the Models page.',
            }
          : null,
      );
    } catch (err) {
      setTransfer({ ok: false, message: errorMessage(err) });
    }
  };

  const checkUpdates = async () => {
    setChecking(true);
    try {
      setUpdate(await window.flow.app.checkForUpdates());
    } catch (err) {
      setUpdate(`The update check failed: ${errorMessage(err)}`);
    } finally {
      setChecking(false);
    }
  };

  return (
    <>
      <PageHeader title="Advanced" />

      <Group
        title="Config file"
        description="Every setting in this window lives in one text file. Edits made in a text editor apply within a second."
      >
        <SettingRow
          label="config.toml"
          description={
            <>
              Open it in your default editor.
              <PathLine path={snapshot.path || info?.configPath} />
            </>
          }
          control={
            <Button onClick={() => void window.flow.config.openFile()}>Open config file</Button>
          }
        />
        <SettingRow
          label="Export or import settings"
          description="Move your settings to another PC. API keys are never included."
          control={
            <>
              <Button onClick={() => void runTransfer('export')}>Export</Button>
              <Button onClick={() => void runTransfer('import')}>Import</Button>
            </>
          }
        >
          <div aria-live="polite">
            {transfer && (
              <Notice tone={transfer.ok ? 'ok' : 'danger'} className="mt-3">
                {transfer.message}
              </Notice>
            )}
          </div>
        </SettingRow>
      </Group>

      <Group
        title="Inserting text"
        description="How the transcript reaches the app you are typing in."
      >
        <SettingRow
          label="Method"
          description="Paste is fast and reliable. Type sends each character as a keystroke, for apps that block paste."
          control={
            <Select
              value={config.insert.method}
              options={METHODS}
              onChange={(v) => set(['insert', 'method'], v)}
            />
          }
        />
        <SettingRow
          label="Paste shortcut"
          description="The keys Flow presses to paste. Per-app exceptions go on the Profiles page."
          disabled={typing}
          control={
            <Select
              value={config.insert.paste_shortcut}
              options={shortcutOptions}
              disabled={typing}
              onChange={(v) => set(['insert', 'paste_shortcut'], v)}
            />
          }
        />
        <SettingRow
          label="Restore clipboard"
          description="Put back whatever you had copied once the text has been pasted."
          disabled={typing}
          control={
            <Toggle
              checked={config.insert.restore_clipboard}
              disabled={typing}
              onChange={(v) => set(['insert', 'restore_clipboard'], v)}
            />
          }
        />
        <SettingRow
          label="Trailing space"
          description="Add a space after each dictation so the next one does not run into it."
          control={
            <Toggle
              checked={config.insert.trailing_space}
              onChange={(v) => set(['insert', 'trailing_space'], v)}
            />
          }
        />
        <SettingRow
          label="Match case"
          description="Start with a lower-case letter when you are dictating into the middle of a sentence."
          control={
            <Toggle
              checked={config.insert.match_case}
              onChange={(v) => set(['insert', 'match_case'], v)}
            />
          }
        />
      </Group>

      <Group title="Troubleshooting">
        <SettingRow
          label="Debug logging"
          description="Writes detailed logs. While this is on, the logs include the text of your dictations."
          control={
            <Toggle
              checked={config.general.debug_logging}
              onChange={(v) => set(['general', 'debug_logging'], v)}
            />
          }
        >
          {config.general.debug_logging && (
            <Notice tone="warn" className="mt-3">
              Logs now contain transcript text. Turn this off when you have finished, and check a
              log before sharing it.
            </Notice>
          )}
        </SettingRow>
        <SettingRow
          label="Logs folder"
          description={
            <>
              Where Flow writes its log files.
              <PathLine path={info?.logsPath} />
            </>
          }
          control={
            <Button onClick={() => void window.flow.app.openLogs()}>Open logs folder</Button>
          }
        />
        <SettingRow
          label="Models folder"
          description={
            <>
              Where downloaded model files are stored.
              <PathLine path={info?.modelsPath} />
            </>
          }
          control={
            <Button onClick={() => void window.flow.app.openModelsFolder()}>
              Open models folder
            </Button>
          }
        />
        <SettingRow
          label="Updates"
          description={
            <span aria-live="polite">
              {update ?? 'Flow checks GitHub Releases and installs signed updates only.'}
            </span>
          }
          control={
            <Button disabled={checking} onClick={() => void checkUpdates()}>
              {checking ? 'Checking…' : 'Check for updates'}
            </Button>
          }
        />
      </Group>

      <About />
    </>
  );
}
