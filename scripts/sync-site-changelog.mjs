#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';

const DEFAULT_SOURCE = 'https://raw.githubusercontent.com/alibaba/anolisa/main/src/anolisa/CHANGELOG.md';
const DEFAULT_INDEX = 'index.html';
const DEFAULT_LIMIT = 5;
const execFileAsync = promisify(execFile);

function parseArgs(argv) {
  const args = {
    source: process.env.CHANGELOG_SOURCE || DEFAULT_SOURCE,
    index: DEFAULT_INDEX,
    limit: DEFAULT_LIMIT,
    dryRun: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--source') args.source = argv[++i];
    else if (arg === '--index') args.index = argv[++i];
    else if (arg === '--limit') args.limit = Number(argv[++i]);
    else if (arg === '--dry-run') args.dryRun = true;
    else if (arg === '--help' || arg === '-h') {
      console.log(`Usage: node scripts/sync-site-changelog.mjs [options]

Options:
  --source <path-or-url>  CHANGELOG source (default: ${DEFAULT_SOURCE})
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
    const res = await fetch(source);
    if (!res.ok) throw new Error(`failed to fetch ${source}: ${res.status} ${res.statusText}`);
    return res.text();
  }
  return readFile(source, 'utf8');
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

function parseReleases(markdown) {
  const english = markdown.split(/\n---\n/)[0];
  const heading = /^## \[([^\]]+)\](?: - ([0-9]{4}-[0-9]{2}-[0-9]{2}))?\s*$/gm;
  const matches = [...english.matchAll(heading)];
  const releases = [];

  for (let i = 0; i < matches.length; i += 1) {
    const match = matches[i];
    const version = match[1];
    if (/^unreleased$/i.test(version)) continue;

    const start = match.index + match[0].length;
    const end = i + 1 < matches.length ? matches[i + 1].index : english.length;
    releases.push({
      version,
      date: match[2] || '',
      sections: parseSections(english.slice(start, end)),
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

function updateBadge(html, version) {
  const badgePattern = /(<div class="badge"><span class="badge-dot"><\/span>ANOLISA v)[^<]+(<\/div>)/;
  if (!badgePattern.test(html)) {
    throw new Error('could not find ANOLISA version badge in index.html');
  }
  return html.replace(badgePattern, `$1${version}$2`);
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
    `- Update homepage version badge to v${version}`,
    `- Add ANOLISA CLI ${version} release notes`,
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
  const changelog = await readSource(args.source);
  const releases = parseReleases(changelog).slice(0, args.limit);
  if (releases.length === 0) throw new Error('no released versions found in changelog');

  const indexPath = path.resolve(args.index);
  let html = await readFile(indexPath, 'utf8');
  html = updateBadge(html, releases[0].version);
  html = replaceReleaseList(html, releases);

  const summary = `sync-site-changelog: ${releases.length} releases, latest ${releases[0].version}`;
  const commitInfo = await buildCommitInfo(releases[0].version);
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
