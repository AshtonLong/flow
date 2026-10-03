/**
 * Shows a binding as key caps with a Record button. Recording uses the global
 * hook in the main process, so Win, lone modifiers and mouse buttons work.
 */
import { useEffect, useRef, useState } from 'react';
import { RotateCcw } from 'lucide-react';
import { normalizeBinding } from '@shared/hotkeys';
import { errorMessage } from '../lib/format';
import { Button, IconButton } from './Button';
import { KeyCaps } from './KeyCaps';

/** Only one recorder listens at a time. */
let activeCancel: (() => void) | null = null;

export function HotkeyRecorder({
  value,
  defaultValue,
  onChange,
  actionName,
}: {
  value: string;
  /** Enables the reset button when given. */
  defaultValue?: string;
  onChange: (binding: string) => void;
  /** For screen readers: "Hold to talk". */
  actionName: string;
}) {
  const [recording, setRecording] = useState(false);
  const [partial, setPartial] = useState('');
  const [error, setError] = useState<string | null>(null);
  const recordingRef = useRef(false);
  const recordButton = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);

  const cancel = () => {
    if (recordingRef.current) void window.flow.hotkeys.cancelCapture();
  };

  const start = async () => {
    activeCancel?.();
    activeCancel = cancel;
    recordingRef.current = true;
    setRecording(true);
    setPartial('');
    setError(null);
    const off = window.flow.hotkeys.onCaptureProgress(setPartial);
    try {
      const result = await window.flow.hotkeys.capture();
      if (result !== null) {
        const binding = normalizeBinding(result);
        if (binding) onChange(binding);
        else setError(`Flow cannot use “${result}” as a hotkey.`);
      }
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      off();
      recordingRef.current = false;
      if (activeCancel === cancel) activeCancel = null;
      restoreFocus.current = true;
      setRecording(false);
    }
  };

  // While recording, keys belong to the recorder: they must not click buttons,
  // move focus or scroll the page.
  useEffect(() => {
    if (!recording) return;
    const swallow = (e: KeyboardEvent) => e.preventDefault();
    const onBlur = () => cancel();
    window.addEventListener('keydown', swallow, true);
    window.addEventListener('keyup', swallow, true);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', swallow, true);
      window.removeEventListener('keyup', swallow, true);
      window.removeEventListener('blur', onBlur);
    };
  }, [recording]);

  useEffect(() => {
    if (!recording && restoreFocus.current) {
      restoreFocus.current = false;
      recordButton.current?.focus();
    }
  }, [recording]);

  useEffect(() => () => cancel(), []);

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-2">
        {recording ? (
          <>
            <span className="flex items-center gap-2 text-fg-2" aria-live="polite">
              {partial ? <KeyCaps binding={partial} /> : null}
              <span>{partial ? 'Release to save' : 'Press keys… release to save'}</span>
            </span>
            <Button onClick={cancel}>Cancel</Button>
          </>
        ) : (
          <>
            <KeyCaps binding={value} />
            <Button
              ref={recordButton}
              onClick={() => void start()}
              aria-label={`Record a new hotkey for ${actionName}`}
            >
              Record
            </Button>
            {defaultValue !== undefined && (
              <IconButton
                label={`Reset to ${defaultValue}`}
                icon={<RotateCcw size={15} />}
                disabled={normalizeBinding(value) === normalizeBinding(defaultValue)}
                onClick={() => onChange(defaultValue)}
              />
            )}
          </>
        )}
      </div>
      {error && (
        <span role="alert" className="text-caption text-danger">
          {error}
        </span>
      )}
    </div>
  );
}
