// Renders the app and tray icons from SVG. Run with Electron:
//   pnpm exec electron scripts/make-icons.mjs
import { app, BrowserWindow } from 'electron';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const outDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '../resources/icons');

// The mark from the marketing site's favicon: three sound bars, then the text cursor.
const MARK = `
  <rect x="6" y="13" width="3" height="6" rx="1.5"/>
  <rect x="11" y="9" width="3" height="14" rx="1.5"/>
  <rect x="16" y="11" width="3" height="10" rx="1.5"/>
  <rect x="22" y="7" width="4" height="18" rx="1"/>`;

function svg({ background, mark = '#000000', inset = 0 }) {
  const size = 32 - inset * 2;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="100%" height="100%">
    <rect x="${inset}" y="${inset}" width="${size}" height="${size}" rx="${(8 * size) / 32}" fill="${background}"/>
    <g fill="${mark}" transform="translate(${inset} ${inset}) scale(${size / 32})">${MARK}</g>
  </svg>`;
}

const VOICE = '#ff5a1f';
const APP = svg({ background: VOICE, inset: 1 });
// Tray sizes are tiny, so the square runs to the edge.
const TRAY = svg({ background: VOICE });
const TRAY_PAUSED = svg({ background: '#9aa0a9', mark: '#3b3f45' });

async function render(markup, size) {
  const win = new BrowserWindow({
    width: size,
    height: size,
    show: false,
    frame: false,
    transparent: true,
    webPreferences: { offscreen: true },
  });
  const html = `<html><body style="margin:0;background:transparent;overflow:hidden">${markup}</body></html>`;
  const file = path.join(os.tmpdir(), `flow-icon-${size}.html`);
  fs.writeFileSync(file, html);
  await win.loadFile(file);
  fs.rmSync(file, { force: true });
  await new Promise((resolve) => setTimeout(resolve, 120));
  const image = await win.webContents.capturePage();
  win.destroy();
  return image.resize({ width: size, height: size, quality: 'best' }).toPNG();
}

/** An .ico is a directory of images; modern Windows accepts PNG-compressed entries. */
function ico(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = 6 + dir.length;
  entries.forEach(({ size, png }, i) => {
    const at = i * 16;
    dir.writeUInt8(size >= 256 ? 0 : size, at);
    dir.writeUInt8(size >= 256 ? 0 : size, at + 1);
    dir.writeUInt16LE(1, at + 4);
    dir.writeUInt16LE(32, at + 6);
    dir.writeUInt32LE(png.length, at + 8);
    dir.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.png)]);
}

app.disableHardwareAcceleration();
// Windows are created and destroyed one at a time; do not quit between them.
app.on('window-all-closed', () => {});
app.whenReady().then(async () => {
  fs.mkdirSync(outDir, { recursive: true });
  const sizes = [16, 24, 32, 48, 64, 128, 256];
  const entries = [];
  for (const size of sizes) entries.push({ size, png: await render(APP, size) });
  fs.writeFileSync(path.join(outDir, 'icon.ico'), ico(entries));
  fs.writeFileSync(path.join(outDir, 'icon.png'), entries.at(-1).png);
  for (const [name, markup] of [
    ['tray', TRAY],
    ['tray-paused', TRAY_PAUSED],
  ]) {
    fs.writeFileSync(path.join(outDir, `${name}.png`), await render(markup, 16));
    fs.writeFileSync(path.join(outDir, `${name}@2x.png`), await render(markup, 32));
    fs.writeFileSync(path.join(outDir, `${name}@3x.png`), await render(markup, 48));
  }
  console.log(`icons written to ${outDir}`);
  app.quit();
});
