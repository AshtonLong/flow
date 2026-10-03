/**
 * The one preload script. Sandboxed preloads must be a single self-contained
 * file, so both windows share it and each gets only its own API, selected by
 * the `--flow-window=` argument the main process passes.
 */
import { exposeOverlayApi } from './overlay';
import { exposeSettingsApi } from './settings';

const kind = process.argv.find((arg) => arg.startsWith('--flow-window='))?.split('=')[1];

if (kind === 'overlay') exposeOverlayApi();
else if (kind === 'settings') exposeSettingsApi();
