import Head from '@docusaurus/Head';
import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import CopyCommand from '../components/CopyCommand';
import SiteLink from '../components/SiteLink';
import {installCommand, type Locale} from '../../content.config';

const content = {
  en: {
    badge: 'Agentic OS 1.0',
    lead: 'An operating system layer built for agents.',
    capabilities: 'Sandbox isolation. Kernel observability. Token efficiency.',
    deployment: 'Managed as one system and deployed through one entry point.',
    statement:
      'The users of operating systems have changed. ANOLISA makes agents first-class participants at the system layer.',
    installLabel: 'One entry point. Enable what you need.',
    agentLabel: 'Bring your Agent in',
    agentPrompt:
      'Read https://agentic-os.sh/agents/ to learn how to use ANOLISA, then help me install it for this environment.',
    copy: 'Copy',
    copied: 'Copied',
    surfaceLabel: 'ANOLISA system surface',
    surfaceStatus: 'online',
    surfaceFooter: 'one entry · capabilities on demand',
    scenariosTitle: 'Solve the critical problems in Agent operations.',
    scenariosIntro:
      'Everyday Agent operations depend on terminal collaboration, Token efficiency, and execution environments. Each capability can be installed independently and integrated on demand.',
    openGuide: 'Open the guide',
    exploreTitle: 'Documentation and project resources',
    exploreIntro:
      'Find user guides, developer documentation, release history, and the machine-readable Agent entry point.',
  },
  zh: {
    badge: 'Agentic OS 1.0',
    lead: 'Agent 原生的操作系统层。',
    capabilities: '沙箱隔离、内核可观测、Token 优化。',
    deployment: '由一个系统统一管理，通过一个入口完成部署。',
    statement: '操作系统的使用者已经改变。ANOLISA 让 Agent 成为系统中的一等公民。',
    installLabel: '一个入口，按需启用',
    agentLabel: '让你的 Agent 接入',
    agentPrompt:
      '阅读 https://agentic-os.sh/agents/，了解如何使用 ANOLISA，并根据当前环境帮我完成安装。',
    copy: '复制',
    copied: '已复制',
    surfaceLabel: 'ANOLISA 系统能力视图',
    surfaceStatus: '在线',
    surfaceFooter: '一个入口 · 能力按需接入',
    scenariosTitle: '解决 Agent 运行中的关键问题。',
    scenariosIntro:
      'Agent 的日常运行，绕不开终端协作、Token 开销和执行环境。每项能力都可独立安装、按需接入。',
    openGuide: '打开指南',
    exploreTitle: '文档与项目资源',
    exploreIntro: '查阅用户指南、开发文档、版本记录和面向 Agent 的机器入口。',
  },
} as const;

const scenarios = {
  en: [
    {
      role: 'TERMINAL COLLABORATION',
      name: 'Cosh-ng',
      title: 'Bring Agent collaboration back to the terminal.',
      surfaceTitle: 'Let the Agent work inside your terminal.',
      proof: 'Linux entry point for the Agent era',
      surfaceBody:
        'Follow live output, run commands, and take control back at any time.',
      body:
        'Cosh-ng runs inside the Shell you already use. Natural-language tasks, tools, Skills, Hooks, MCP, and multiple models are available directly from the terminal without opening a separate chat window.',
      promise:
        'Work still happens in the Shell. The Agent simply adds another layer of capability.',
      cta: 'Explore Cosh-ng',
      components: [
        {
          label: 'cosh-ng',
          href: '/docs/user-guide/user-entrypoint/cosh-ng/quickstart',
        },
        {
          label: 'Cosh',
          href: '/docs/user-guide/user-entrypoint/copilot-shell/quickstart',
        },
      ],
      href: '/docs/user-guide/user-entrypoint/cosh-ng/quickstart',
      accent: 'cyan',
    },
    {
      role: 'TOKEN & CONTEXT',
      name: 'Token Flow',
      title: 'Measure Token use. Reduce it.',
      surfaceTitle: 'Keep your harness. Start saving Tokens with one command.',
      proof: '30–70% tool-output compression*',
      surfaceBody:
        'Supports Claude Code, Codex, Qoder, and more; original content stays retrievable.',
      body:
        'From context compression and memory reuse to usage tracking, Token Flow makes Agent Token consumption more efficient and transparent, and integrates into existing workflows on demand.',
      promise:
        'No Agent code changes. The savings remain visible in the statistics.',
      cta: 'Start with Tokenless',
      components: [
        {
          label: 'Tokenless',
          href: '/docs/user-guide/token-saving/tokenless/quickstart',
        },
        {
          label: 'Agent Memory',
          href: '/docs/user-guide/token-saving/agent-memory/quickstart',
        },
        {
          label: 'AgentSight',
          href: '/docs/user-guide/agent-observability/agentsight',
        },
      ],
      href: '/docs/user-guide/token-saving/tokenless/quickstart',
      accent: 'lime',
    },
    {
      role: 'SANDBOX EXECUTION',
      name: 'Sandbox Infra',
      title: 'Give Agents an isolated, recoverable execution environment.',
      surfaceTitle: 'Run risky work in an isolated, recoverable sandbox.',
      proof: 'Runtime · ms snapshots / rollback',
      surfaceBody:
        'Track changes, enforce boundaries, and return to a checkpoint when needed.',
      body:
        'ANOLISA manages sandbox environments, Agent Sec Core isolates risky commands, ws-ckpt keeps recovery points for workspace changes, and SkillFS mounts Skills on demand. Each capability can run independently or as part of the same runtime.',
      promise:
        'From environment preparation and risk isolation to change recovery, Agent execution gets a clear system boundary.',
      cta: 'Explore ANOLISA CLI',
      components: [
        {
          label: 'ANOLISA CLI',
          href: '/docs/user-guide/user-entrypoint/anolisa-cli',
        },
        {
          label: 'Agent Sec Core',
          href: '/docs/user-guide/agent-security/agent-sec-core/quickstart',
        },
        {
          label: 'ws-ckpt',
          href: '/docs/user-guide/runtime/ws-ckpt',
        },
        {
          label: 'SkillFS',
          href: '/docs/user-guide/runtime/skillfs',
        },
        {
          label: 'Blaze',
          href: 'https://github.com/alibaba/anolisa/tree/main/src/blaze',
        },
      ],
      href: '/docs/user-guide/user-entrypoint/anolisa-cli',
      accent: 'amber',
    },
  ],
  zh: [
    {
      role: '终端协作',
      name: 'Cosh-ng',
      title: '把 Agent 协作带回终端',
      surfaceTitle: '让 Agent 直接在你的终端里工作',
      proof: 'AI Agent 时代重构 Linux 操作入口',
      surfaceBody: '跟随终端输出、执行命令，你可以随时接管。',
      body:
        'Cosh-ng 就运行在熟悉的 Shell 里。自然语言任务、工具、Skills、Hooks、MCP 和多种模型都能从终端直接调用，不需要另开一个聊天窗口。',
      promise: '工作仍在 Shell 里完成，Agent 只是多了一层能力。',
      cta: '了解 Cosh-ng',
      components: [
        {
          label: 'cosh-ng',
          href: '/docs/user-guide/user-entrypoint/cosh-ng/quickstart',
        },
        {
          label: 'Cosh',
          href: '/docs/user-guide/user-entrypoint/copilot-shell/quickstart',
        },
      ],
      href: '/docs/user-guide/user-entrypoint/cosh-ng/quickstart',
      accent: 'cyan',
    },
    {
      role: 'TOKEN 与上下文',
      name: 'Token Flow',
      title: '让 Token 消耗可测、可省',
      surfaceTitle: '不换 Harness，一行命令开始节省 Token',
      proof: '30–70% 工具输出压缩*',
      surfaceBody: '支持 Claude Code、Codex、Qoder 等，压缩后仍可取回原文。',
      body:
        '从上下文压缩、记忆复用到开销追踪，Token Flow 让 Agent 的 Token 使用更高效、更透明，并可按需接入现有工作流。',
      promise: '不改 Agent 代码，省了多少 Token 在统计里看得见。',
      cta: '从 Tokenless 开始',
      components: [
        {
          label: 'Tokenless',
          href: '/docs/user-guide/token-saving/tokenless/quickstart',
        },
        {
          label: 'Agent Memory',
          href: '/docs/user-guide/token-saving/agent-memory/quickstart',
        },
        {
          label: 'AgentSight',
          href: '/docs/user-guide/agent-observability/agentsight',
        },
      ],
      href: '/docs/user-guide/token-saving/tokenless/quickstart',
      accent: 'lime',
    },
    {
      role: '沙箱执行',
      name: 'Sandbox Infra',
      title: '为 Agent 提供可隔离、可恢复的执行环境',
      surfaceTitle: '让高风险任务隔离运行，也能随时恢复',
      proof: 'Runtime · 毫秒级工作区快照 / 回滚',
      surfaceBody: '约束权限、记录操作，失败时回到检查点。',
      body:
        'ANOLISA 管理沙箱环境，Agent Sec Core 隔离高风险命令，ws-ckpt 为工作区变更保留恢复点，SkillFS 按需挂载 Skills。各项能力既可独立启用，也可以组合使用。',
      promise: '从环境准备、风险隔离到变更恢复，为 Agent 执行提供清晰的系统边界。',
      cta: '查看 ANOLISA CLI',
      components: [
        {
          label: 'ANOLISA CLI',
          href: '/docs/user-guide/user-entrypoint/anolisa-cli',
        },
        {
          label: 'Agent Sec Core',
          href: '/docs/user-guide/agent-security/agent-sec-core/quickstart',
        },
        {
          label: 'ws-ckpt',
          href: '/docs/user-guide/runtime/ws-ckpt',
        },
        {
          label: 'SkillFS',
          href: '/docs/user-guide/runtime/skillfs',
        },
        {
          label: 'Blaze',
          href: 'https://github.com/alibaba/anolisa/tree/main/src/blaze',
        },
      ],
      href: '/docs/user-guide/user-entrypoint/anolisa-cli',
      accent: 'amber',
    },
  ],
} as const;

const routes = {
  en: [
    {
      label: 'USER GUIDE',
      title: 'Use ANOLISA',
      body: 'Install, configure, operate, and troubleshoot each capability.',
      href: '/docs/user-guide',
    },
    {
      label: 'DEVELOPER GUIDE',
      title: 'Build with ANOLISA',
      body: 'Read architecture, protocols, extension points, and test guidance.',
      href: '/docs/developer-guide',
    },
    {
      label: 'CHANGELOG',
      title: 'Follow releases',
      href: '/changelog',
    },
    {
      label: 'FOR AGENTS',
      title: 'Read the machine entry point',
      body: 'Let your Agent read and understand ANOLISA.',
      href: '/agents/',
    },
  ],
  zh: [
    {
      label: '用户指南',
      title: '使用 ANOLISA',
      body: '查找各项能力的安装、配置、运行和故障排查说明。',
      href: '/docs/user-guide',
    },
    {
      label: '开发者指南',
      title: '参与 ANOLISA 开发',
      body: '阅读架构、协议、扩展点与测试说明。',
      href: '/docs/developer-guide',
    },
    {
      label: 'CHANGELOG',
      title: '了解版本变化',
      href: '/changelog',
    },
    {
      label: 'FOR AGENTS',
      title: '打开 Agent 入口',
      body: '让你的 Agent 读取并了解 ANOLISA。',
      href: '/agents/',
    },
  ],
} as const;

export default function Home() {
  const {i18n} = useDocusaurusContext();
  const locale: Locale = i18n.currentLocale === 'zh' ? 'zh' : 'en';
  const t = content[locale];
  const scenarioItems = scenarios[locale];
  const routeItems = routes[locale];
  const heroGlyph = useBaseUrl('/img/brand/anolisa-glyph-signal.svg');

  return (
    <Layout title={t.lead} description={`${t.capabilities} ${t.deployment}`}>
      <Head>
        <meta property="og:type" content="website" />
      </Head>
      <main className="homePage">
        <section className="heroSection homeSnapPoint">
          <div className="heroGrid siteContainer">
            <div className="heroCopy">
              <div className="releaseBadge">
                <span aria-hidden="true" />
                {t.badge}
              </div>
              <div className="heroIdentity">
                <img src={heroGlyph} alt="" aria-hidden="true" />
                <h1>ANOLISA</h1>
              </div>
              <div className="heroPositioning">
                <p>{t.lead}</p>
                <p>{t.capabilities}</p>
                <p>{t.deployment}</p>
              </div>
              <p className="heroStatement">{t.statement}</p>

              <div className="heroCommand">
                <p>{t.installLabel}</p>
                <CopyCommand command={installCommand} label={t.copy} copiedLabel={t.copied} />
              </div>

              <div className="agentCommand">
                <p><span aria-hidden="true">&gt;</span> {t.agentLabel}</p>
                <CopyCommand command={t.agentPrompt} label={t.copy} copiedLabel={t.copied} />
              </div>
            </div>

            <aside className="systemSurface" aria-label={t.surfaceLabel}>
              <header>
                <span>anolisa://system-surface</span>
                <span className="surfaceStatus">{t.surfaceStatus}</span>
              </header>
              <div className="surfaceCore">
                <span className="surfaceCoreLabel">AGENT WORKLOAD</span>
                <strong>ANOLISA</strong>
                <span>OBSERVE · CONTROL · RECOVER</span>
              </div>
              {scenarioItems.map((scenario) => (
                <SiteLink
                  locale={locale}
                  to={scenario.href}
                  className={`surfaceLayer surfaceLayer--${scenario.accent}`}
                  key={scenario.name}>
                  <div className="surfaceLayerMeta">
                    <span>{scenario.role}</span>
                    {'proof' in scenario && <small>{scenario.proof}</small>}
                  </div>
                  <strong>{scenario.surfaceTitle}</strong>
                  <em>{scenario.surfaceBody}</em>
                  <b aria-hidden="true">→</b>
                </SiteLink>
              ))}
              <footer>
                <span>{t.surfaceFooter}</span>
                <span>source: main</span>
              </footer>
            </aside>
          </div>
        </section>

        <section className="scenarioSection homeSnapPoint" id="scenarios">
          <div className="siteContainer">
            <div className="scenarioHeading">
              <div>
                <h2>{t.scenariosTitle}</h2>
                <p>{t.scenariosIntro}</p>
              </div>
              <SiteLink locale={locale} to="/docs/quickstart" className="textLink">
                {t.openGuide} →
              </SiteLink>
            </div>

            <div className="scenarioGrid">
              {scenarioItems.map((scenario) => (
                <article
                  className={`scenarioCard scenarioCard--${scenario.accent}`}
                  key={scenario.name}>
                  <header>
                    <span>{scenario.role}</span>
                    <b aria-hidden="true">↗</b>
                  </header>
                  <p className="scenarioName">{scenario.name}</p>
                  <h3>
                    <SiteLink locale={locale} to={scenario.href} className="scenarioTitleLink">
                      {scenario.title}
                    </SiteLink>
                  </h3>
                  <p className="scenarioBody">{scenario.body}</p>
                  <p className="scenarioPromise">{scenario.promise}</p>
                  <SiteLink locale={locale} to={scenario.href} className="scenarioCta">
                    {scenario.cta} →
                  </SiteLink>
                  <div className="componentTags">
                    {scenario.components.map((component) => (
                      <SiteLink locale={locale} to={component.href} key={component.label}>
                        {component.label}
                      </SiteLink>
                    ))}
                  </div>
                </article>
              ))}
            </div>

            <div className="routeSection">
              <div className="routeIntro">
                <h2>{t.exploreTitle}</h2>
                <p>{t.exploreIntro}</p>
              </div>
              <div className="routeGrid">
                {routeItems.map((route) => (
                  <SiteLink locale={locale} to={route.href} className="routeCard" key={route.label}>
                    <span>{route.label}</span>
                    <strong>{route.title}</strong>
                    {'body' in route && <p>{route.body}</p>}
                    <b aria-hidden="true">→</b>
                  </SiteLink>
                ))}
              </div>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
