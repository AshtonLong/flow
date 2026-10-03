/**
 * Microphone level meters. Levels arrive ~60 times a second, so the bar is
 * updated through a ref rather than React state.
 */
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { CaptureError, startCapture, toCaptureError } from '../../common/capture';
import { Button } from './Button';
import { Notice } from './Notice';

export interface LevelMeterHandle {
  set(level: number): void;
}

export const LevelMeter = forwardRef<LevelMeterHandle, { label: string }>(function LevelMeter(
  { label },
  ref,
) {
  const track = useRef<HTMLDivElement>(null);
  const fill = useRef<HTMLDivElement>(null);
  const lastAnnounced = useRef(0);
  useImperativeHandle(ref, () => ({
    set(level: number) {
      const clamped = Math.min(1, Math.max(0, level));
      // The raw RMS is tiny for normal speech; a square root reads more like loudness.
      if (fill.current) fill.current.style.transform = `scaleX(${Math.sqrt(clamped)})`;
      const now = performance.now();
      if (track.current && now - lastAnnounced.current > 400) {
        lastAnnounced.current = now;
        track.current.setAttribute('aria-valuenow', String(Math.round(Math.sqrt(clamped) * 100)));
      }
    },
  }));
  return (
    <div
      ref={track}
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={0}
      className="meter-track"
    >
      <div ref={fill} className="meter-fill" />
    </div>
  );
});

const HELP: Record<CaptureError['reason'], string> = {
  'device-missing': 'This microphone is not connected. Plug it in, or pick another one.',
  permission:
    'Windows is blocking microphone access. In Windows Settings, open Privacy & security > Microphone and allow desktop apps.',
  busy: 'Another app has exclusive use of this microphone. Close it, then try again.',
  unknown: 'The microphone could not be opened.',
};

export function captureHelp(error: CaptureError): string {
  return error.reason === 'unknown' && error.message
    ? `${HELP.unknown} ${error.message}`
    : HELP[error.reason];
}

/**
 * Opens the microphone and shows its live level for as long as it is mounted.
 * The device is released on unmount and whenever `device` changes.
 */
export function MicMeter({
  device,
  onListening,
}: {
  /** A microphone label, or `"default"`. */
  device: string;
  /** Called once audio is flowing, e.g. to refresh device labels. */
  onListening?: () => void;
}) {
  const meter = useRef<LevelMeterHandle>(null);
  const [error, setError] = useState<CaptureError | null>(null);
  const [listening, setListening] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const onListeningRef = useRef(onListening);
  onListeningRef.current = onListening;

  useEffect(() => {
    let cancelled = false;
    let release: (() => void) | null = null;
    setError(null);
    setListening(false);
    startCapture({
      device,
      onChunk: () => {},
      onLevel: (level) => meter.current?.set(level),
    })
      .then((handle) => {
        if (cancelled) {
          handle.dispose();
          return;
        }
        release = () => handle.dispose();
        handle.onEnded = () => {
          handle.dispose();
          setListening(false);
          setError(new CaptureError('device-missing', 'Microphone was disconnected'));
        };
        setListening(true);
        onListeningRef.current?.();
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(toCaptureError(err));
      });
    return () => {
      cancelled = true;
      release?.();
    };
  }, [device, attempt]);

  if (error) {
    return (
      <Notice
        tone="danger"
        action={
          <Button size="sm" onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </Button>
        }
      >
        {captureHelp(error)}
      </Notice>
    );
  }
  return (
    <div className="flex items-center gap-3">
      <span className="w-[76px] shrink-0 text-caption text-fg-2" aria-hidden="true">
        {listening ? 'Input level' : 'Opening…'}
      </span>
      <div className="flex-1">
        <LevelMeter ref={meter} label="Microphone input level" />
      </div>
    </div>
  );
}
