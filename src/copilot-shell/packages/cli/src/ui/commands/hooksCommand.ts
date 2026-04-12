/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type {
  SlashCommand,
  SlashCommandActionReturn,
  CommandContext,
  MessageActionReturn,
} from './types.js';
import { CommandKind } from './types.js';
import { t } from '../../i18n/index.js';
import type { HookRegistryEntry } from '@copilot-shell/core';
import { HookEventName, HookType } from '@copilot-shell/core';
import { SettingScope } from '../../config/settings.js';
import process from 'node:process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Format hook source for display
 */
function formatHookSource(source: string): string {
  switch (source) {
    case 'project':
      return 'Project';
    case 'user':
      return 'User';
    case 'system':
      return 'System';
    case 'extensions':
      return 'Extension';
    default:
      return source;
  }
}

/**
 * Format hook status for display
 */
function formatHookStatus(enabled: boolean): string {
  return enabled ? '✓ Enabled' : '✗ Disabled';
}

const listCommand: SlashCommand = {
  name: 'list',
  get description() {
    return t('List all configured hooks');
  },
  kind: CommandKind.BUILT_IN,
  action: async (
    context: CommandContext,
    _args: string,
  ): Promise<MessageActionReturn> => {
    const { config } = context.services;
    if (!config) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Config not loaded.'),
      };
    }

    const hookSystem = config.getHookSystem();
    if (!hookSystem) {
      return {
        type: 'message',
        messageType: 'info',
        content: t(
          'Hooks are not enabled. Enable hooks in settings to use this feature.',
        ),
      };
    }

    const registry = hookSystem.getRegistry();
    const allHooks = registry.getAllHooks();

    if (allHooks.length === 0) {
      return {
        type: 'message',
        messageType: 'info',
        content: t(
          'No hooks configured. Add hooks in your settings.json file.',
        ),
      };
    }

    // Group hooks by event
    const hooksByEvent = new Map<string, HookRegistryEntry[]>();
    for (const hook of allHooks) {
      const eventName = hook.eventName;
      if (!hooksByEvent.has(eventName)) {
        hooksByEvent.set(eventName, []);
      }
      hooksByEvent.get(eventName)!.push(hook);
    }

    let output = `**Configured Hooks (${allHooks.length} total)**\n\n`;

    for (const [eventName, hooks] of hooksByEvent) {
      output += `### ${eventName}\n`;
      for (const hook of hooks) {
        const name = hook.config.name || hook.config.command || 'unnamed';
        const source = formatHookSource(hook.source);
        const status = formatHookStatus(hook.enabled);
        const matcher = hook.matcher ? ` (matcher: ${hook.matcher})` : '';
        output += `- **${name}** [${source}] ${status}${matcher}\n`;
      }
      output += '\n';
    }

    return {
      type: 'message',
      messageType: 'info',
      content: output,
    };
  },
};

const enableCommand: SlashCommand = {
  name: 'enable',
  get description() {
    return t('Enable a disabled hook');
  },
  kind: CommandKind.BUILT_IN,
  action: async (
    context: CommandContext,
    args: string,
  ): Promise<MessageActionReturn> => {
    const hookName = args.trim();
    if (!hookName) {
      return {
        type: 'message',
        messageType: 'error',
        content: t(
          'Please specify a hook name. Usage: /hooks enable <hook-name>',
        ),
      };
    }

    const { config } = context.services;
    if (!config) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Config not loaded.'),
      };
    }

    const hookSystem = config.getHookSystem();
    if (!hookSystem) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Hooks are not enabled.'),
      };
    }

    const registry = hookSystem.getRegistry();
    registry.setHookEnabled(hookName, true);

    return {
      type: 'message',
      messageType: 'info',
      content: t('Hook "{{name}}" has been enabled for this session.', {
        name: hookName,
      }),
    };
  },
  completion: async (context: CommandContext, partialArg: string) => {
    const { config } = context.services;
    if (!config) return [];

    const hookSystem = config.getHookSystem();
    if (!hookSystem) return [];

    const registry = hookSystem.getRegistry();
    const allHooks = registry.getAllHooks();

    // Return disabled hooks for enable command (deduplicated by name)
    const disabledHookNames = allHooks
      .filter((hook) => !hook.enabled)
      .map((hook) => hook.config.name || hook.config.command || '')
      .filter((name) => name && name.startsWith(partialArg));
    return [...new Set(disabledHookNames)];
  },
};

const disableCommand: SlashCommand = {
  name: 'disable',
  get description() {
    return t('Disable an active hook');
  },
  kind: CommandKind.BUILT_IN,
  action: async (
    context: CommandContext,
    args: string,
  ): Promise<MessageActionReturn> => {
    const hookName = args.trim();
    if (!hookName) {
      return {
        type: 'message',
        messageType: 'error',
        content: t(
          'Please specify a hook name. Usage: /hooks disable <hook-name>',
        ),
      };
    }

    const { config } = context.services;
    if (!config) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Config not loaded.'),
      };
    }

    const hookSystem = config.getHookSystem();
    if (!hookSystem) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Hooks are not enabled.'),
      };
    }

    const registry = hookSystem.getRegistry();
    registry.setHookEnabled(hookName, false);

    return {
      type: 'message',
      messageType: 'info',
      content: t('Hook "{{name}}" has been disabled for this session.', {
        name: hookName,
      }),
    };
  },
  completion: async (context: CommandContext, partialArg: string) => {
    const { config } = context.services;
    if (!config) return [];

    const hookSystem = config.getHookSystem();
    if (!hookSystem) return [];

    const registry = hookSystem.getRegistry();
    const allHooks = registry.getAllHooks();

    // Return enabled hooks for disable command (deduplicated by name)
    const enabledHookNames = allHooks
      .filter((hook) => hook.enabled)
      .map((hook) => hook.config.name || hook.config.command || '')
      .filter((name) => name && name.startsWith(partialArg));
    return [...new Set(enabledHookNames)];
  },
};

const installCommand: SlashCommand = {
  name: 'install',
  get description() {
    return t('Install sandbox-guard hooks into user settings');
  },
  kind: CommandKind.BUILT_IN,
  action: async (
    context: CommandContext,
    args: string,
  ): Promise<MessageActionReturn> => {
    const { config, settings } = context.services;
    if (!config) {
      return {
        type: 'message',
        messageType: 'error',
        content: t('Config not loaded.'),
      };
    }

    // Determine script paths
    const scriptArg = args.trim();
    const homeDir = process.env['HOME'] || process.env['USERPROFILE'] || '~';
    const hooksDir = `${homeDir}/.copilot-shell/hooks`;

    // Locate bundled hooks directory:
    //   Production: $COSH_DATA_DIR/hooks/  (set by cosh-wrapper.sh at install time)
    //   Development (esbuild bundle): dist/hooks/  (adjacent to dist/cli.js)
    //   Development (tsc output):     packages/cli/dist/hooks/  (3 levels up)
    let bundledHooksDir: string;
    const dataDir = process.env['COSH_DATA_DIR'];
    if (dataDir) {
      bundledHooksDir = path.join(dataDir, 'hooks');
    } else {
      const thisFile = fileURLToPath(import.meta.url);
      const distDir = path.dirname(thisFile);
      const bundleHooksCandidate = path.join(distDir, 'hooks');
      const tscHooksCandidate = path.join(distDir, '..', '..', '..', 'hooks');
      bundledHooksDir = fs.existsSync(bundleHooksCandidate)
        ? bundleHooksCandidate
        : tscHooksCandidate;
    }

    // Resolve effective script paths:
    //   Priority: explicit CLI arg > ~/.copilot-shell/hooks/ (after copy) > bundled fallback
    // If the user dir script doesn't exist AND the bundled script does, use bundled path
    // directly so hooks always work even without a successful copy.
    function resolveScriptPath(scriptName: string, override?: string): string {
      if (override) return override;
      const userPath = path.join(hooksDir, scriptName);
      if (fs.existsSync(userPath)) return userPath;
      const bundledPath = path.join(bundledHooksDir, scriptName);
      if (fs.existsSync(bundledPath)) return bundledPath;
      return userPath; // fall back to user path (will be copied below)
    }

    // Ensure user hooks directory exists and copy bundled scripts (always overwrite)
    const copyErrors: string[] = [];
    try {
      fs.mkdirSync(hooksDir, { recursive: true });
      for (const script of ['sandbox-guard.py', 'sandbox-failure-handler.py']) {
        const dest = path.join(hooksDir, script);
        const src = path.join(bundledHooksDir, script);
        if (fs.existsSync(src)) {
          fs.copyFileSync(src, dest);
          fs.chmodSync(dest, 0o755);
        } else {
          copyErrors.push(`bundled script not found: ${src}`);
        }
      }
    } catch (err) {
      copyErrors.push(String(err));
    }

    const guardScript = resolveScriptPath(
      'sandbox-guard.py',
      scriptArg || undefined,
    );
    const failureScript = resolveScriptPath('sandbox-failure-handler.py');
    const preToolUseEntry = {
      hooks: [
        {
          type: 'command',
          command: `python3 ${guardScript}`,
          name: 'sandbox-guard',
        },
      ],
    };
    const postToolUseFailureEntry = {
      hooks: [
        {
          type: 'command',
          command: `python3 ${failureScript}`,
          name: 'sandbox-failure-handler',
        },
      ],
    };

    // Read current user hooks config (if any)
    const currentHooks =
      (
        settings.forScope(SettingScope.User).settings as Record<string, unknown>
      )['hooks'] ?? {};
    const hooksConfig = (currentHooks ?? {}) as Record<string, unknown>;

    // Check idempotency: skip if sandbox-guard is already registered
    const preToolUseList = (hooksConfig['PreToolUse'] as unknown[]) ?? [];
    const alreadyInstalled = preToolUseList.some(
      (entry: unknown) =>
        Array.isArray((entry as Record<string, unknown>)['hooks']) &&
        ((entry as Record<string, unknown>)['hooks'] as unknown[]).some(
          (h: unknown) =>
            (h as Record<string, unknown>)['name'] === 'sandbox-guard',
        ),
    );

    if (!alreadyInstalled) {
      // Merge new hooks into the existing config
      const newHooksConfig = {
        ...hooksConfig,
        PreToolUse: [...preToolUseList, preToolUseEntry],
        PostToolUseFailure: [
          ...((hooksConfig['PostToolUseFailure'] as unknown[]) ?? []),
          postToolUseFailureEntry,
        ],
      };
      settings.setValue(SettingScope.User, 'hooks', newHooksConfig);
    }

    // Activate hooks in the current session via dynamic registration
    const hookSystem = config.getHookSystem();
    if (hookSystem) {
      hookSystem.registerHook(HookEventName.PreToolUse, {
        type: HookType.Command,
        command: `python3 ${guardScript}`,
        name: 'sandbox-guard',
      });
      hookSystem.registerHook(HookEventName.PostToolUseFailure, {
        type: HookType.Command,
        command: `python3 ${failureScript}`,
        name: 'sandbox-failure-handler',
      });
      // Enable both hooks
      hookSystem.setHookEnabled('sandbox-guard', true);
      hookSystem.setHookEnabled('sandbox-failure-handler', true);
    }

    const alreadyMsg = alreadyInstalled
      ? t(' (already in settings, session hooks refreshed)')
      : '';
    const copyErrMsg =
      copyErrors.length > 0
        ? `\n⚠ Script copy warnings:\n${copyErrors.map((e) => `  - ${e}`).join('\n')}`
        : '';

    return {
      type: 'message',
      messageType: copyErrors.length > 0 ? 'error' : 'info',
      content: t(
        'sandbox-guard installed{{already}}\n- PreToolUse: {{guard}}\n- PostToolUseFailure: {{failure}}{{copyErr}}',
        {
          already: alreadyMsg,
          guard: guardScript,
          failure: failureScript,
          copyErr: copyErrMsg,
        },
      ),
    };
  },
};

export const hooksCommand: SlashCommand = {
  name: 'hooks',
  get description() {
    return t('Manage Cosh hooks');
  },
  kind: CommandKind.BUILT_IN,
  subCommands: [listCommand, enableCommand, disableCommand, installCommand],
  action: async (
    context: CommandContext,
    args: string,
  ): Promise<SlashCommandActionReturn> => {
    // If no subcommand provided, show list
    if (!args.trim()) {
      const result = await listCommand.action?.(context, '');
      return result ?? { type: 'message', messageType: 'info', content: '' };
    }

    const [subcommand, ...rest] = args.trim().split(/\s+/);
    const subArgs = rest.join(' ');

    let result: SlashCommandActionReturn | void;
    switch (subcommand.toLowerCase()) {
      case 'list':
        result = await listCommand.action?.(context, subArgs);
        break;
      case 'enable':
        result = await enableCommand.action?.(context, subArgs);
        break;
      case 'disable':
        result = await disableCommand.action?.(context, subArgs);
        break;
      case 'install':
        result = await installCommand.action?.(context, subArgs);
        break;
      default:
        return {
          type: 'message',
          messageType: 'error',
          content: t(
            'Unknown subcommand: {{cmd}}. Available: list, enable, disable, install',
            {
              cmd: subcommand,
            },
          ),
        };
    }
    return result ?? { type: 'message', messageType: 'info', content: '' };
  },
  completion: async (context: CommandContext, partialArg: string) => {
    const subcommands = ['list', 'enable', 'disable', 'install'];
    const parts = partialArg.split(/\s+/);

    if (parts.length <= 1) {
      // Complete subcommand
      return subcommands.filter((cmd) => cmd.startsWith(partialArg));
    }

    // Complete subcommand arguments
    const [subcommand, ...rest] = parts;
    const subArgs = rest.join(' ');

    switch (subcommand.toLowerCase()) {
      case 'enable':
        return enableCommand.completion?.(context, subArgs) ?? [];
      case 'disable':
        return disableCommand.completion?.(context, subArgs) ?? [];
      default:
        return [];
    }
  },
};
