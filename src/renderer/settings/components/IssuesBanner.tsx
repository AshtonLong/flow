import { useId, useState } from 'react';
import { ChevronDown, TriangleAlert } from 'lucide-react';
import { useConfig } from '../lib/config';
import { plural } from '../lib/format';
import { formatPath } from '../lib/patch';
import { Button } from './Button';

/** Slim warning shown on every page while `config.toml` has invalid values. */
export function IssuesBanner() {
  const { snapshot } = useConfig();
  const [open, setOpen] = useState(false);
  const listId = useId();
  const issues = snapshot.issues;
  if (issues.length === 0) return null;
  return (
    <div
      className="mb-5 rounded-md border border-line bg-warn-soft"
      role="region"
      aria-label="Config file problems"
    >
      <div className="flex items-center gap-2.5 py-1.5 pr-1.5 pl-3">
        <TriangleAlert size={16} aria-hidden="true" className="shrink-0 text-warn" />
        <p className="min-w-0 flex-1">config.toml has {plural(issues.length, 'problem')}</p>
        <Button size="sm" variant="subtle" onClick={() => void window.flow.config.openFile()}>
          Open config file
        </Button>
        <Button
          size="sm"
          variant="subtle"
          aria-expanded={open}
          aria-controls={listId}
          onClick={() => setOpen((v) => !v)}
        >
          {open ? 'Hide' : 'Show'}
          <ChevronDown
            size={14}
            aria-hidden="true"
            className={`transition-transform ${open ? 'rotate-180' : ''}`}
          />
        </Button>
      </div>
      {open && (
        <div id={listId} className="border-t border-line px-3 py-2 text-caption">
          <p className="pb-1 text-fg-2">
            Each of these falls back to its default until the file is fixed.
          </p>
          <ul className="selectable">
            {issues.map((issue, i) => (
              <li key={i} className="flex gap-4 py-1">
                <code className="w-[220px] shrink-0 font-mono break-all">
                  {formatPath(issue.path)}
                </code>
                <span className="min-w-0 flex-1">{issue.message}</span>
                <span className="w-[56px] shrink-0 text-right text-fg-2">
                  {issue.line !== undefined ? `line ${issue.line}` : ''}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
