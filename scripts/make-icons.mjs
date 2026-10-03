// Renders the app and tray icons from SVG. Run with Electron:
//   pnpm exec electron scripts/make-icons.mjs
import { app, BrowserWindow } from 'electron';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const outDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '../resources/icons');

const BARS = [
  { x: 64, h: 44 },
  { x: 96, h: 104 },
  { x: 128, h: 148 },
  { x: 160, h: 84 },
  { x: 192, h: 40 },
];

function svg({ background, bar, radius = 58, inset = 12, barWidth = 18 }) {
  const size = 256 - inset * 2;
  const bars = BARS.map(
    ({ x, h }) =>
      `<rect x="${x - barWidth / 2}" y="${128 - h / 2}" width="${barWidth}" height="${h}" rx="${barWidth / 2}" fill="${bar}"/>`,
  ).join('');
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="100%" height="100%">
    <rect x="${inset}" y="${inset}" width="${size}" height="${size}" rx="${radius}" fill="${background}"/>
    ${bars}
  </svg>`;
}

const APP = svg({ background: '#2563eb', bar: '#ffffff' });
// Tray sizes are tiny: fill the square and thicken the bars so they survive at 16 px.
const TRAY = svg({ background: '#2563eb', bar: '#ffffff', inset: 0, radius: 60, barWidth: 24 });
const TRAY_PAUSED = svg({
  background: '#6b7280',
  bar: '#d1d5db',
  inset: 0,
  radius: 60,
  barWidth: 24,
});

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
