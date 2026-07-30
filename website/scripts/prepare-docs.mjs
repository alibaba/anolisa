import {mkdir, readFile, rm, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {
  exists,
  generatedDir,
  repoRoot,
  titleFromMarkdown,
  toPosix,
  walkFiles,
  websiteDir,
} from './lib.mjs';

const repository = 'https://github.com/alibaba/anolisa';
const siteUrl = process.env.SITE_URL ?? 'https://agentic-os.sh';
const baseUrl = process.env.BASE_URL ?? '/';
const docsOutput = path.join(generatedDir, 'docs');
const i18nOutput = path.join(generatedDir, 'i18n', 'zh', 'docusaurus-plugin-content-docs', 'current');

function normalizedTarget(relativePath) {
  const parsed = path.posix.parse(toPosix(relativePath));
  const basename = parsed.base === 'README.md' ? 'index.md' : parsed.base.toLowerCase();
  return path.posix.join(parsed.dir, basename);
}

function publicDocumentPath(target) {
  const withoutExtension = target.replace(/\.md$/, '');
  if (withoutExtension === 'index') return '';
  return withoutExtension.replace(/\/index$/, '');
}

async function sourceDocuments(locale) {
  const suffix = locale === 'zh' ? '_zh' : '';
  const documents = [
    {source: `docs/README${suffix}.md`, target: 'index.md', position: 1},
    {source: `docs/QUICKSTART${suffix}.md`, target: 'quickstart.md', position: 2},
    {source: `docs/BUILDING${suffix}.md`, target: 'building.md', position: 3},
  ];
  for (const section of ['user-guide', 'developer-guide']) {
    const root = path.join(repoRoot, 'docs', section, locale);
    for (const file of await walkFiles(root, (candidate) => candidate.endsWith('.md'))) {
      documents.push({
        source: toPosix(path.relative(repoRoot, file)),
        target: path.posix.join(section, normalizedTarget(path.relative(root, file))),
      });
    }
  }
  return documents;
}

const englishDocuments = await sourceDocuments('en');
const chineseDocuments = await sourceDocuments('zh');
const publicPaths = new Map();
for (const document of englishDocuments) {
  publicPaths.set(document.source, `/docs/${publicDocumentPath(document.target)}`);
}
for (const document of chineseDocuments) {
  publicPaths.set(document.source, `/zh/docs/${publicDocumentPath(document.target)}`);
}

function knownAlias(source, unresolvedPath) {
  const locale = source.includes('/zh/') || source.endsWith('_zh.md') ? 'zh' : 'en';
  if (unresolvedPath.endsWith('/copilot-shell.md')) {
    return `docs/user-guide/${locale}/user-entrypoint/copilot-shell/QUICKSTART.md`;
  }
  if (unresolvedPath.endsWith('/copilot-shell/overview.md')) {
    return `docs/user-guide/${locale}/user-entrypoint/copilot-shell/QUICKSTART.md`;
  }
  if (unresolvedPath.includes('/user-entrypoint/developers/')) {
    const basename = path.posix.basename(unresolvedPath);
    if (source.includes('/cosh-ng/')) return `docs/developer-guide/${locale}/cosh-ng/${basename}`;
    if (source.includes('/copilot-shell/')) {
      return `docs/developer-guide/${locale}/copilot-shell/hooks/${basename}`;
    }
  }
  return undefined;
}

async function rewriteLinks(markdown, source) {
  const sourceDirectory = path.posix.dirname(source);
  const replacements = [];
  const linkPattern = /(!?)\[([^\]]*)\]\(([^)]+)\)/g;
  for (const match of markdown.matchAll(linkPattern)) {
    const rawTarget = match[3].trim();
    if (/^(?:[a-z]+:|#|\/)/i.test(rawTarget)) continue;
    const [targetWithoutHash, hash = ''] = rawTarget.split('#', 2);
    if (!targetWithoutHash.endsWith('.md')) continue;

    let resolved = path.posix.normalize(path.posix.join(sourceDirectory, targetWithoutHash));
    if (!(await exists(path.join(repoRoot, resolved)))) {
      resolved = knownAlias(source, resolved) || resolved;
    }

    let replacement;
    if (publicPaths.has(resolved)) {
      replacement = `${publicPaths.get(resolved)}${hash ? `#${hash}` : ''}`;
      const sourceIsChinese = source.includes('/zh/') || source.endsWith('_zh.md');
      const targetIsChinese = replacement.startsWith('/zh/');
      if (sourceIsChinese && targetIsChinese) {
        replacement = replacement.replace(/^\/zh/, '');
      } else if (sourceIsChinese !== targetIsChinese) {
        replacement = new URL(`${baseUrl}${replacement.replace(/^\//, '')}`, siteUrl).toString();
      }
    } else if (await exists(path.join(repoRoot, resolved))) {
      replacement = `${repository}/blob/main/${resolved}${hash ? `#${hash}` : ''}`;
    }
    if (replacement) {
      replacements.push({start: match.index, end: match.index + match[0].length, value: `${match[1]}[${match[2]}](${replacement})`});
    }
  }
  let output = markdown;
  for (const replacement of replacements.reverse()) {
    output = output.slice(0, replacement.start) + replacement.value + output.slice(replacement.end);
  }
  return output;
}

function makeMdxSafe(markdown) {
  let inFence = false;
  return markdown
    .split('\n')
    .map((line) => {
      if (/^\s*(```|~~~)/.test(line)) {
        inFence = !inFence;
        return line;
      }
      if (inFence) return line;
      return line
        .replace(/\\`/g, '&#96;')
        .split(/(`+[^`]*`+)/g)
        .map((segment, index) => {
          if (index % 2 === 1) {
            const content = segment
              .replace(/^`+|`+$/g, '')
              .replace(/&#96;/g, '`')
              .replace(/(?<!\\)\|/g, '\\|');
            const delimiter = content.includes('`') ? '``' : '`';
            return `${delimiter}${content}${delimiter}`;
          }
          return segment.replace(/</g, '&lt;').replace(/\{/g, '&#123;').replace(/\}/g, '&#125;');
        })
        .join('');
    })
    .join('\n');
}

function frontMatter(document, markdown) {
  const publicPath = publicDocumentPath(document.target);
  const slug = publicPath ? `/${publicPath}` : '/';
  const title = titleFromMarkdown(markdown, path.posix.basename(slug));
  const fields = [
    '---',
    `title: ${JSON.stringify(title)}`,
    `slug: ${JSON.stringify(slug)}`,
    `sidebar_label: ${JSON.stringify(title)}`,
    `custom_edit_url: ${JSON.stringify(`${repository}/edit/main/${document.source}`)}`,
  ];
  const position = document.position ?? documentPositions[document.target];
  if (position) fields.push(`sidebar_position: ${position}`);
  if (document.target.endsWith('/index.md')) fields.push('sidebar_position: 1');
  if (document.target === 'user-guide/index.md') fields.push('displayed_sidebar: userGuide');
  if (document.target === 'developer-guide/index.md') fields.push('displayed_sidebar: developerGuide');
  fields.push('---', '');
  return fields.join('\n');
}

const categoryNames = {
  en: {
    'user-guide': 'User Guide',
    'developer-guide': 'Developer Guide',
    'user-entrypoint': 'User Entry Points',
    'agent-observability': 'Observability',
    'agent-security': 'Security',
    'token-saving': 'Token Efficiency',
    runtime: 'Runtime',
    cli: 'CLI', core: 'Core', shell: 'Shell', hooks: 'Hooks',
  },
  zh: {
    'user-guide': '用户指南',
    'developer-guide': '开发者指南',
    'user-entrypoint': '用户入口',
    'agent-observability': '可观测性',
    'agent-security': '安全',
    'token-saving': 'Token 效率',
    runtime: '运行时',
    cli: 'CLI', core: '核心', shell: 'Shell', hooks: 'Hooks',
  },
};

// Sidebar ordering mirrors the architecture layers: entry points → token
// saving → runtime → the cross-cutting observability/security layer.
// Without explicit positions Docusaurus sorts categories alphabetically,
// which reverses that reading order.
const categoryPositions = {
  'user-guide/user-entrypoint': 3,
  'user-guide/token-saving': 4,
  'user-guide/runtime': 5,
  'user-guide/agent-observability': 6,
  'user-guide/agent-security': 7,
};

const documentPositions = {
  'user-guide/installation.md': 2,
  'user-guide/troubleshooting.md': 8,
};

function humanize(segment) {
  return segment
    .split('-')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}

async function writeCategories(outputRoot, documents, locale) {
  const directories = new Set();
  for (const document of documents) {
    let directory = path.posix.dirname(document.target);
    while (directory !== '.') {
      directories.add(directory);
      directory = path.posix.dirname(directory);
    }
  }
  for (const directory of [...directories].sort()) {
    const segment = path.posix.basename(directory);
    const label = categoryNames[locale][segment] || humanize(segment);
    const indexId = `${directory}/index`;
    const hasIndex = documents.some((document) => document.target === `${indexId}.md`);
    const topLevelPosition =
      directory === 'user-guide' ? 3 : directory === 'developer-guide' ? 4 : categoryPositions[directory];
    const metadata = {
      label,
      key: `category-${directory.replaceAll('/', '-')}`,
      ...(topLevelPosition ? {position: topLevelPosition} : {}),
    };
    if (hasIndex) metadata.link = {type: 'doc', id: indexId};
    const output = path.join(outputRoot, directory, '_category_.json');
    await mkdir(path.dirname(output), {recursive: true});
    await writeFile(output, `${JSON.stringify(metadata, null, 2)}\n`);
  }
}

async function prepareLocale(documents, outputRoot, locale) {
  for (const document of documents) {
    const sourceMarkdown = await readFile(path.join(repoRoot, document.source), 'utf8');
    const markdown = makeMdxSafe(await rewriteLinks(sourceMarkdown, document.source));
    const output = path.join(outputRoot, document.target);
    await mkdir(path.dirname(output), {recursive: true});
    await writeFile(output, `${frontMatter(document, sourceMarkdown)}${markdown}`);
  }
  await writeCategories(outputRoot, documents, locale);
}

await rm(docsOutput, {recursive: true, force: true});
await rm(path.join(generatedDir, 'i18n'), {recursive: true, force: true});
await prepareLocale(englishDocuments, docsOutput, 'en');
await prepareLocale(chineseDocuments, i18nOutput, 'zh');

const translationRoot = path.join(generatedDir, 'i18n', 'zh');
const themeTranslationRoot = path.join(translationRoot, 'docusaurus-theme-classic');
await mkdir(themeTranslationRoot, {recursive: true});
await writeFile(
  path.join(themeTranslationRoot, 'navbar.json'),
  `${JSON.stringify(
    {
      title: {message: 'ANOLISA', description: 'The title in the navbar'},
      'item.label.Docs': {message: '文档', description: 'Navbar item with label Docs'},
      'item.label.User Guide': {message: '用户指南', description: 'Navbar item with label User Guide'},
      'item.label.Developer Guide': {message: '开发者指南', description: 'Navbar item with label Developer Guide'},
      'item.label.Changelog': {message: '变更日志', description: 'Navbar item with label Changelog'},
      'item.label.For Agents': {message: 'Agent 入口', description: 'Navbar item with label For Agents'},
      'item.label.GitHub': {message: 'GitHub', description: 'Navbar item with label GitHub'},
    },
    null,
    2,
  )}\n`,
);
await writeFile(
  path.join(themeTranslationRoot, 'footer.json'),
  `${JSON.stringify(
    {
      'link.title.Docs': {message: '文档', description: 'Footer column title'},
      'link.title.Guides': {message: '指南', description: 'Footer column title'},
      'link.title.Community': {message: '社区', description: 'Footer column title'},
      'link.item.label.Documentation': {message: '文档首页', description: 'Footer link label'},
      'link.item.label.Quickstart': {message: '快速开始', description: 'Footer link label'},
      'link.item.label.Building': {message: '源码构建', description: 'Footer link label'},
      'link.item.label.Changelog': {message: '变更日志', description: 'Footer link label'},
      'link.item.label.User Guide': {message: '用户指南', description: 'Footer link label'},
      'link.item.label.Developer Guide': {message: '开发者指南', description: 'Footer link label'},
      'link.item.label.For Agents': {message: 'Agent 入口', description: 'Footer link label'},
      'link.item.label.GitHub': {message: 'GitHub', description: 'Footer link label'},
      'link.item.label.Contributing': {message: '参与贡献', description: 'Footer link label'},
      'link.item.label.Security': {message: '安全', description: 'Footer link label'},
      copyright: {message: `Copyright © ${new Date().getFullYear()} ANOLISA 贡献者。Apache-2.0。`, description: 'Footer copyright'},
    },
    null,
    2,
  )}\n`,
);

console.log(`Prepared ${englishDocuments.length} English and ${chineseDocuments.length} Chinese documents in ${path.relative(websiteDir, generatedDir)}.`);
