/** Auto-update from GitHub Releases. Only installed (packaged) builds update. */
import { app } from 'electron';
import electronUpdater from 'electron-updater';
import { log } from './log';

let wired = false;

function updater() {
  const { autoUpdater } = electronUpdater;
  if (!wired) {
    wired = true;
    autoUpdater.autoDownload = true;
    autoUpdater.autoInstallOnAppQuit = true;
    autoUpdater.logger = null;
    autoUpdater.on('error', (err) => log.warn('updater', String(err)));
    autoUpdater.on('update-downloaded', (info) =>
      log.info('updater', `update ${info.version} downloaded; installs on quit`),
    );
  }
  return autoUpdater;
}

/** Checks for an update and returns one line describing the result. */
export async function checkForUpdates(): Promise<string> {
  if (!app.isPackaged) return 'Updates are checked in installed builds only.';
  try {
    const result = await updater().checkForUpdates();
    const latest = result?.updateInfo.version;
    if (!latest || latest === app.getVersion()) return `Flow ${app.getVersion()} is up to date.`;
    return `Flow ${latest} is available and will install when you quit.`;
  } catch (err) {
    log.warn('updater', String(err));
    return 'Could not check for updates. Check your connection and try again.';
  }
}

/** Quiet background check shortly after launch. */
export function scheduleUpdateCheck(): void {
  if (!app.isPackaged) return;
  setTimeout(() => void checkForUpdates(), 30_000).unref();
}
