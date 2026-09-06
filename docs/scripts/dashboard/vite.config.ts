import { createHash } from 'node:crypto';
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import preact from '@preact/preset-vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig, type Plugin } from 'vite';

// The files whose bytes decide what dist/ contains. test_pulse_dashboard.py
// recomputes the same hash over the same set and fails when dist/ is stale.
export const HASHED_FILES = ['index.html', 'package.json', 'package-lock.json', 'tsconfig.json', 'vite.config.ts', 'eslint.config.js'];

function walk(dir: string, out: string[]): string[] {
  for (const name of readdirSync(dir).sort()) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

export function sourceHash(root: string): string {
  const files = [...HASHED_FILES.map((f) => join(root, f)), ...walk(join(root, 'src'), [])]
    .map((f) => relative(root, f))
    .sort();
  const h = createHash('sha256');
  for (const f of files) {
    h.update(f);
    h.update('\0');
    h.update(readFileSync(join(root, f)));
    h.update('\0');
  }
  return h.digest('hex');
}

function version(pkg: string): string {
  return JSON.parse(readFileSync(join(__dirname, 'node_modules', pkg, 'package.json'), 'utf8')).version as string;
}

// BUILD.json: the source hash and tool versions the build came from. No
// timestamp, so the same tree always builds the same bytes (evaluator P2).
function buildStamp(): Plugin {
  return {
    name: 'pulse-build-stamp',
    apply: 'build',
    closeBundle() {
      const stamp = {
        schema: 'pulse-hub-build/1',
        source_hash: sourceHash(__dirname),
        files: ['index.html', 'app.js', 'app.css'],
        tools: { vite: version('vite'), preact: version('preact'), tailwindcss: version('tailwindcss'), typescript: version('typescript') },
      };
      writeFileSync(join(__dirname, 'dist', 'BUILD.json'), JSON.stringify(stamp, null, 1) + '\n');
    },
  };
}

export default defineConfig({
  base: '/',
  plugins: [preact(), tailwindcss(), buildStamp()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsDir: '',
    target: 'es2022',
    cssCodeSplit: false,
    sourcemap: false,
    modulePreload: { polyfill: false },
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
        entryFileNames: 'app.js',
        chunkFileNames: 'app.js',
        assetFileNames: 'app.[ext]',
      },
    },
  },
});
