#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';

const DEFAULT_INDEX = 'index.html';
const DEFAULT_LIMIT = 5;
const RAW_BASE = 'https://raw.githubusercontent.com/alibaba/anolisa/main';
const BLOB_BASE = 'https://github.com/alibaba/anolisa/blob/main';
const COMPONENTS = [
  {
    id: 'anolisa',
    label: 'ANOLISA CLI',
    summary: 'Command-line runtime, installer, and agent operating layer.',
    path: 'src/anolisa/CHANGELOG.md',
  },
  {
    id: 'copilot-shell',
    label: 'Copilot Shell',
    summary: 'Terminal-native agent shell and interactive CLI.',
    path: 'src/copilot-shell/CHANGELOG.md',
  },
  {
    id: 'agent-sec-core',
    label: 'AgentSecCore',
    summary: 'Security kernel, scanners, adapters, and policy enforcement.',
    path: 'src/agent-sec-core/CHANGELOG.md',
  },
  {
    id: 'agentsight',
    label: 'AgentSight',
    summary: 'Agent observability, tracing, dashboard, and interruption insights.',
    path: 'src/agentsight/CHANGELOG.md',
  },
  {
    id: 'agent-memory',
    label: 'Agent Memory',
    summary: 'Filesystem memory server, indexing, snapshots, and MCP tools.',
    path: 'src/agent-memory/CHANGELOG.md',
  },
  {
    id: 'os-skills',
    label: 'OS Skills',
    summary: 'Install skills and operating-system level agent workflows.',
    path: 'src/os-skills/CHANGELOG.md',
  },
  {
    id: 'tokenless',
    label: 'Tokenless',
    summary: 'Prompt compression, token optimization, and agent adapters.',
    path: 'src/tokenless/CHANGELOG.md',
  },
  {
    id: 'ws-ckpt',
    label: 'WS Ckpt',
    summary: 'Workspace checkpoint, rollback, and policy controls.',
    path: 'src/ws-ckpt/CHANGELOG.md',
  },
];
const FALLBACK_REFS = ['up/main', 'github/main', 'internal/main', 'main'];
const execFileAsync = promisify(execFile);

function parseArgs(argv) {
  const args = {
    index: DEFAULT_INDEX,
    limit: DEFAULT_LIMIT,
    dryRun: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--index') args.index = argv[++i];
    else if (arg === '--limit') args.limit = Number(argv[++i]);
    else if (arg === '--dry-run') args.dryRun = true;
    else if (arg === '--help' || arg === '-h') {
      console.log(`Usage: node scripts/sync-site-changelog.mjs [options]

Options:
  --index <path>          Site HTML file to update (default: ${DEFAULT_INDEX})
  --limit <n>             Number of released versions to render (default: ${DEFAULT_LIMIT})
  --dry-run               Print summary without writing files`);
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }

  if (!Number.isInteger(args.limit) || args.limit <= 0) {
    throw new Error('--limit must be a positive integer');
  }

  return args;
}

async function readSource(source) {
  if (/^https?:\/\//.test(source)) {
    return fetchText(source);
  }
  return readFile(source, 'utf8');
}

function rawUrl(pathName) {
  return `${RAW_BASE}/${pathName}`;
}

function blobUrl(pathName) {
  return `${BLOB_BASE}/${pathName}`;
}

function sleep(ms) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

async function fetchText(url) {
  const attempts = 3;
  let lastError = null;

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`failed to fetch ${url}: ${res.status} ${res.statusText}`);
      return res.text();
    } catch (error) {
      lastError = error;
      if (attempt < attempts) await sleep(500 * attempt);
    }
  }

  throw lastError;
}

async function readComponentChangelog(component) {
  const url = rawUrl(component.path);
  try {
    return await readSource(url);
  } catch (fetchError) {
    for (const ref of FALLBACK_REFS) {
      try {
        const source = `${ref}:${component.path}`;
        const { stdout } = await execFileAsync('git', ['show', source]);
        console.warn(`warning: ${fetchError.message}; using ${source}`);
        return stdout;
      } catch {
        // Try the next local ref.
      }
    }
    throw new Error(`${fetchError.message}; also failed to read local git changelog refs for ${component.id}`);
  }
}

function escapeHtml(value) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function renderInline(markdown) {
  let html = escapeHtml(markdown);
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  return html;
}

function languageBlock(markdown, lang) {
  const parts = markdown.split(/\n---\n/);
  return lang === 'zh' ? parts[1] || '' : parts[0];
}

function parseReleases(markdown, lang = 'en') {
  const content = languageBlock(markdown, lang);
  const heading = /^##\s+(?:\[([^\]]+)\]|([0-9][^\s]*))(?:\s+-\s+([0-9]{4}-[0-9]{2}-[0-9]{2}))?\s*$/gm;
  const matches = [...content.matchAll(heading)];
  const releases = [];

  for (let i = 0; i < matches.length; i += 1) {
    const match = matches[i];
    const version = match[1] || match[2];
    if (/^(unreleased|未发布)$/i.test(version)) continue;

    const start = match.index + match[0].length;
    const end = i + 1 < matches.length ? matches[i + 1].index : content.length;
    releases.push({
      version,
      date: match[3] || '',
      sections: parseSections(content.slice(start, end)),
    });
  }

  return releases;
}

function parseSections(markdown) {
  const sections = [];
  let current = null;
  let currentBullet = null;

  function ensureSection(title) {
    if (!current || current.title !== title) {
      current = { title, items: [] };
      sections.push(current);
    }
    return current;
  }

  function pushBullet() {
    if (currentBullet && current) {
      current.items.push(currentBullet.trim());
    }
    currentBullet = null;
  }

  for (const rawLine of markdown.split('\n')) {
    const line = rawLine.trimEnd();
    if (!line.trim()) {
      pushBullet();
      continue;
    }

    const headingMatch = line.match(/^###\s+(.+)$/);
    if (headingMatch) {
      pushBullet();
      ensureSection(headingMatch[1].trim());
      continue;
    }

    if (/^\|/.test(line)) {
      continue;
    }

    const boldHeadingMatch = line.trim().match(/^\*\*([^*]+)\*\*$/);
    if (boldHeadingMatch) {
      pushBullet();
      ensureSection(boldHeadingMatch[1].trim());
      continue;
    }

    const bulletMatch = line.match(/^\s*-\s+(.+)$/);
    if (bulletMatch) {
      pushBullet();
      ensureSection(current ? current.title : 'Release');
      currentBullet = bulletMatch[1];
      continue;
    }

    if (/^\s+/.test(line) && currentBullet) {
      currentBullet += ` ${line.trim()}`;
      continue;
    }

    ensureSection('Release');
    currentBullet = currentBullet ? `${currentBullet} ${line.trim()}` : line.trim();
  }

  pushBullet();
  return sections.filter((section) => section.items.length > 0);
}

function versionAnchor(version) {
  return `anolisa-cli-${version.replaceAll('.', '-')}`;
}

function renderRelease(release, index) {
  const delay = Math.min(index + 1, 5);
  const sections = release.sections.map((section) => `          <div class="release-body-section">
            <h3>${escapeHtml(section.title)}</h3>
            <ul>
${section.items.map((item) => `              <li>${renderInline(item)}</li>`).join('\n')}
            </ul>
          </div>`).join('\n');

  return `        <article class="release reveal reveal-d${delay}" id="${versionAnchor(release.version)}">
          <div class="release-meta">
            <div class="release-version">${escapeHtml(release.version)}</div>
            <div class="release-date">${escapeHtml(release.date)}</div>
          </div>
          <div class="release-body">
${sections}
          </div>
        </article>`;
}

function renderJsonScript(data) {
  const json = JSON.stringify(data, null, 2).replaceAll('<', '\\u003c');
  return `        <!-- ANOLISA_CHANGELOG_DATA_START -->\n        <script type="application/json" id="changelog-data">${json}</script>\n        <!-- ANOLISA_CHANGELOG_DATA_END -->`;
}

function replaceChangelogData(html, data) {
  const block = renderJsonScript(data);
  const markerPattern = /        <!-- ANOLISA_CHANGELOG_DATA_START -->[\s\S]*?        <!-- ANOLISA_CHANGELOG_DATA_END -->/;
  if (markerPattern.test(html)) {
    return html.replace(markerPattern, block);
  }

  const morePattern = /(\n      <div class="changelog-more">[\s\S]*?      <\/div>)/;
  if (!morePattern.test(html)) {
    throw new Error('could not find changelog more block in index.html');
  }
  return html.replace(morePattern, `$1\n\n${block}`);
}

function replaceReleaseList(html, releases) {
  const generated = releases.map(renderRelease).join('\n\n');
  const block = `        <!-- ANOLISA_CLI_CHANGELOG_START -->\n${generated}\n        <!-- ANOLISA_CLI_CHANGELOG_END -->`;
  const markerPattern = /        <!-- ANOLISA_CLI_CHANGELOG_START -->[\s\S]*?        <!-- ANOLISA_CLI_CHANGELOG_END -->/;

  if (markerPattern.test(html)) {
    return html.replace(markerPattern, block);
  }

  const releaseListPattern = /(<div class="release-list">\n)[\s\S]*?(\n      <\/div>\n\n      <footer class="cap-footer">)/;
  if (!releaseListPattern.test(html)) {
    throw new Error('could not find changelog release list in index.html');
  }
  return html.replace(releaseListPattern, `$1${block}$2`);
}

async function buildChangelogData(limit) {
  const components = [];

  for (const component of COMPONENTS) {
    const markdown = await readComponentChangelog(component);
    const enReleases = parseReleases(markdown, 'en').slice(0, limit);
    const zhReleases = parseReleases(markdown, 'zh').slice(0, limit);
    if (enReleases.length === 0) {
      throw new Error(`no released versions found in ${component.path}`);
    }

    components.push({
      id: component.id,
      label: component.label,
      summary: component.summary,
      sourceUrl: blobUrl(component.path),
      languages: {
        en: { label: 'EN', releases: enReleases },
        zh: { label: '中文', releases: zhReleases },
      },
    });
  }

  return {
    limit,
    latestVersion: components[0].languages.en.releases[0].version,
    components,
  };
}

async function gitConfig(key) {
  try {
    const { stdout } = await execFileAsync('git', ['config', key]);
    return stdout.trim();
  } catch {
    return '';
  }
}

async function buildCommitInfo(version) {
  const title = `docs(site): sync changelog to v${version}`;
  const body = [
    '- Refresh component changelog data from upstream releases',
    '- Add component changelog tabs and language toggle',
  ];
  const name = await gitConfig('user.name');
  const email = await gitConfig('user.email');
  const signedOffBy = name && email
    ? `Signed-off-by: ${name} <${email}>`
    : 'Signed-off-by: <configure git user.name and user.email>';

  return { title, body, signedOffBy };
}

function printCommitInfo({ title, body, signedOffBy }) {
  console.log('');
  console.log('Suggested commit message:');
  console.log(title);
  console.log('');
  body.forEach((line) => console.log(line));
  console.log('');
  console.log(signedOffBy);
  console.log('');
  console.log('Suggested commands:');
  console.log('git add .gitignore index.html scripts/sync-site-changelog.mjs');
  console.log(`git commit -s -m ${JSON.stringify(title)} \\`);
  body.forEach((line, index) => {
    const suffix = index === body.length - 1 ? '' : ' \\';
    console.log(`  -m ${JSON.stringify(line)}${suffix}`);
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const changelogData = await buildChangelogData(args.limit);
  const defaultComponent = changelogData.components[0];
  const defaultReleases = defaultComponent.languages.en.releases;

  const indexPath = path.resolve(args.index);
  let html = await readFile(indexPath, 'utf8');
  html = replaceReleaseList(html, defaultReleases);
  html = replaceChangelogData(html, changelogData);

  const summary = `sync-site-changelog: ${changelogData.components.length} components, latest ${changelogData.latestVersion}`;
  const commitInfo = await buildCommitInfo(changelogData.latestVersion);
  if (args.dryRun) {
    console.log(summary);
    printCommitInfo(commitInfo);
    return;
  }

  await writeFile(indexPath, html);
  console.log(summary);
  printCommitInfo(commitInfo);
}

main().catch((error) => {
  console.error(`error: ${error.message}`);
  process.exit(1);
});
