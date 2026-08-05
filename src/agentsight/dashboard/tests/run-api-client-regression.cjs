const { execFileSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');

const outputDir = mkdtempSync(join(tmpdir(), 'agentsight-api-client-regression-'));

try {
  const typeRoots = join(outputDir, 'types');
  mkdirSync(typeRoots);
  execFileSync(
    'tsc',
    [
      '--typeRoots',
      typeRoots,
      '--outDir',
      outputDir,
      '--module',
      'commonjs',
      '--target',
      'es2020',
      '--lib',
      'es2020,dom',
      'src/utils/apiClient.ts',
      'src/utils/containmentLifecycle.ts',
      'tests/apiClient-globals.d.ts',
    ],
    { stdio: 'inherit' },
  );
  execFileSync('node', ['--test', 'tests/apiClient-regression.test.cjs'], {
    env: {
      ...process.env,
      AGENTSIGHT_API_CLIENT_BUILD: join(outputDir, 'utils', 'apiClient.js'),
      AGENTSIGHT_CONTAINMENT_LIFECYCLE_BUILD: join(outputDir, 'utils', 'containmentLifecycle.js'),
    },
    stdio: 'inherit',
  });
} finally {
  rmSync(outputDir, { force: true, recursive: true });
}
