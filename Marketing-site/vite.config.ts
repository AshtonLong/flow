import { defineConfig, type Plugin } from 'vite';

/** Where the buttons point. Change these when the repository or release URL changes. */
const LINKS: Record<string, string> = {
  repo: 'https://github.com/AshtonLong/flow',
  download: 'https://github.com/AshtonLong/flow/releases/latest',
  licence: 'https://github.com/AshtonLong/flow/blob/main/LICENSE',
  spec: 'https://github.com/AshtonLong/flow/blob/main/SPEC.md',
};

/** Fills `%repo%`-style tokens in index.html and preloads the one font the page uses. */
function sitePlugin(): Plugin {
  return {
    name: 'flow-site',
    transformIndexHtml: {
      order: 'post',
      handler(html, ctx) {
        const filled = html.replace(/%(\w+)%/g, (token, key: string) => LINKS[key] ?? token);
        const font = Object.keys(ctx.bundle ?? {}).find((file) =>
          /recursive-latin-full-normal.*\.woff2$/.test(file),
        );
        return {
          html: filled,
          tags: font
            ? [
                {
                  tag: 'link',
                  attrs: {
                    rel: 'preload',
                    as: 'font',
                    type: 'font/woff2',
                    href: font,
                    crossorigin: '',
                  },
                  injectTo: 'head',
                },
              ]
            : [],
        };
      },
    },
  };
}

export default defineConfig({
  // Relative asset paths, so the build works at a domain root or under a sub-path.
  base: './',
  plugins: [sitePlugin()],
  build: { target: 'es2022' },
  server: { port: 5199, strictPort: true },
});
