import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import type {Locale} from '../../content.config';

const content = {
  en: {
    eyebrow: 'anolisa://agents',
    title: 'Set up Tokenless with your Agent',
    intro:
      'Use the Tokenless setup guide to check compatibility, install the capability, and enable the matching Agent adapter.',
    note:
      'Automated setup currently covers Linux and macOS. Ask before making system changes.',
    endpoints: 'Machine-readable endpoints',
  },
  zh: {
    eyebrow: 'anolisa://agents',
    title: '让 Agent 配置 Tokenless',
    intro:
      '读取 Tokenless 配置指南，检查兼容性、安装能力并启用匹配的 Agent Adapter。',
    note: '自动安装目前面向 Linux 与 macOS；执行系统变更前应先征得用户确认。',
    endpoints: '机器可读入口',
  },
} as const;

export default function AgentsPage() {
  const {i18n} = useDocusaurusContext();
  const locale: Locale = i18n.currentLocale === 'zh' ? 'zh' : 'en';
  const t = content[locale];
  const endpointItems = [
    ['tokenless.txt', useBaseUrl('/agents/tokenless.txt')],
    ['repo-index.json', useBaseUrl('/agents/repo-index.json')],
    ['repo-index.txt', useBaseUrl('/agents/repo-index.txt')],
    ['cli-reference.txt', useBaseUrl('/agents/cli-reference.txt')],
    ['changelog.txt', useBaseUrl('/agents/changelog.txt')],
    ['llms.txt', useBaseUrl('/llms.txt')],
    ['llms-full.txt', useBaseUrl('/llms-full.txt')],
  ];

  return (
    <Layout title="ANOLISA for Agents" description={t.intro}>
      <main className="agentEntry">
        <header className="agentEntryHero">
          <div className="siteContainer narrowContainer">
            <p className="eyebrow">{t.eyebrow}</p>
            <h1>{t.title}</h1>
            <p>{t.intro}</p>
          </div>
        </header>
        <div className="siteContainer narrowContainer agentEntryBody">
          <section>
            <h2>{t.endpoints}</h2>
            <p>{t.note}</p>
            <div className="agentEndpointGrid">
              {endpointItems.map(([label, href]) => (
                <a href={href} key={label}>
                  <span>{label}</span>
                  <b aria-hidden="true">→</b>
                </a>
              ))}
            </div>
          </section>
        </div>
      </main>
    </Layout>
  );
}
