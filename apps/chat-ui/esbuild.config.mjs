import { build } from 'esbuild';
import { solidPlugin } from 'esbuild-plugin-solid';

async function main() {
  await build({
    entryPoints: ['apps/chat-ui/src/index.tsx'],
    bundle: true,
    format: 'esm',
    target: 'es2020',
    sourcemap: true,
    outfile: 'apps/chat-ui/dist/main.js',
    logLevel: 'info',
    jsx: 'automatic',
    jsxImportSource: 'solid-js',
    plugins: [solidPlugin()],
    define: {
      'process.env.NODE_ENV': JSON.stringify('production'),
    },
    loader: {
      '.wasm': 'file',
    },
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
