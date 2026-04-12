/**
 * @license
 * Copyright 2025 Qwen
 * SPDX-License-Identifier: Apache-2.0
 */

import {
  CommandKind,
  type CommandCompletionItem,
  type CommandContext,
  type SlashCommand,
} from './types.js';
import { MessageType, type HistoryItemSkillsList } from '../types.js';
import { t } from '../../i18n/index.js';
import { AsyncFzf } from 'fzf';
import type { SkillConfig } from '@copilot-shell/core';
/** FUTURE: Re-enable once remote registry is available
const cacheClearSubCommand: SlashCommand = {
  name: 'clear',
  get description() {
    return t('Clear remote skill cache.');
  },
  kind: CommandKind.BUILT_IN,
  action: async (context: CommandContext) => {
    const skillManager = context.services.config?.getSkillManager();
    if (!skillManager) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Could not retrieve skill manager.'),
        },
        Date.now(),
      );
      return;
    }

    try {
      await skillManager.clearRemoteCache();
      context.ui.addItem(
        {
          type: MessageType.INFO,
          text: t('Remote skill cache cleared successfully.'),
        },
        Date.now(),
      );
    } catch (error) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Failed to clear cache: {{error}}', {
            error: error instanceof Error ? error.message : String(error),
          }),
        },
        Date.now(),
      );
    }
  },
};

const cacheSubCommand: SlashCommand = {
  name: 'cache',
  get description() {
    return t('Manage remote skill cache.');
  },
  kind: CommandKind.BUILT_IN,
  subCommands: [cacheClearSubCommand],
  action: async (context: CommandContext) => {
    context.ui.addItem(
      {
        type: MessageType.INFO,
        text: t('Usage: /skills cache clear'),
      },
      Date.now(),
    );
  },
};

const remoteSubCommand: SlashCommand = {
  name: 'remote',
  get description() {
    return t('List available remote skills.');
  },
  kind: CommandKind.BUILT_IN,
  action: async (context: CommandContext) => {
    const skillManager = context.services.config?.getSkillManager();
    if (!skillManager) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Could not retrieve skill manager.'),
        },
        Date.now(),
      );
      return;
    }

    try {
      const remoteSkills = await skillManager.listRemoteSkills();
      if (remoteSkills.length === 0) {
        context.ui.addItem(
          {
            type: MessageType.INFO,
            text: t('No remote skills available.'),
          },
          Date.now(),
        );
        return;
      }
      const skillsListItem: HistoryItemSkillsList = {
        type: MessageType.SKILLS_LIST,
        skills: remoteSkills.map((skill) => ({
          name: skill.name,
          description: skill.description,
        })),
      };
      context.ui.addItem(skillsListItem, Date.now());
    } catch (error) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Failed to fetch remote skills: {{error}}', {
            error: error instanceof Error ? error.message : String(error),
          }),
        },
        Date.now(),
      );
    }
  },
};
*/

export const skillsCommand: SlashCommand = {
  name: 'skills',
  get description() {
    return t('List available skills.');
  },
  kind: CommandKind.BUILT_IN,
  subCommands: [], // _remoteSubCommand and _cacheSubCommand are disabled (remote registry unavailable)
  action: async (context: CommandContext, args?: string) => {
    const argParts = (args?.trim() ?? '').split(/\s+/);
    const skillName = argParts[0] ?? '';

    const skillManager = context.services.config?.getSkillManager();
    if (!skillManager) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Could not retrieve skill manager.'),
        },
        Date.now(),
      );
      return;
    }

    // Include remote skills in validation
    const skills = await skillManager.listSkills({ includeRemote: true });
    if (skills.length === 0) {
      context.ui.addItem(
        {
          type: MessageType.INFO,
          text: t('No skills are currently available.'),
        },
        Date.now(),
      );
      return;
    }

    if (!skillName) {
      const sortedSkills = [...skills].sort((left, right) =>
        left.name.localeCompare(right.name),
      );
      const skillsListItem: HistoryItemSkillsList = {
        type: MessageType.SKILLS_LIST,
        skills: sortedSkills.map((skill) => ({ name: skill.name })),
      };
      context.ui.addItem(skillsListItem, Date.now());
      return;
    }

    const normalizedName = skillName.toLowerCase();
    const hasSkill = skills.some(
      (skill) => skill.name.toLowerCase() === normalizedName,
    );

    if (!hasSkill) {
      context.ui.addItem(
        {
          type: MessageType.ERROR,
          text: t('Unknown skill: {{name}}', { name: skillName }),
        },
        Date.now(),
      );
      return;
    }

    const rawInput = context.invocation?.raw ?? `/skills ${skillName}`;
    return {
      type: 'submit_prompt',
      content: [{ text: rawInput }],
    };
  },
  completion: async (
    context: CommandContext,
    partialArg: string,
  ): Promise<CommandCompletionItem[]> => {
    const skillManager = context.services.config?.getSkillManager();
    if (!skillManager) {
      return [];
    }

    // Include remote skills in completion
    const skills = await skillManager.listSkills({ includeRemote: true });
    const normalizedPartial = partialArg.trim();
    const matches = await getSkillMatches(skills, normalizedPartial);

    return matches.map((skill) => ({
      value: skill.name,
      description: skill.description,
    }));
  },
};

async function getSkillMatches(
  skills: SkillConfig[],
  query: string,
): Promise<SkillConfig[]> {
  if (!query) {
    return skills;
  }

  const names = skills.map((skill) => skill.name);
  const skillMap = new Map(skills.map((skill) => [skill.name, skill]));

  try {
    const fzf = new AsyncFzf(names, {
      fuzzy: 'v2',
      casing: 'case-insensitive',
    });
    const results = (await fzf.find(query)) as Array<{ item: string }>;
    return results
      .map((result) => skillMap.get(result.item))
      .filter((skill): skill is SkillConfig => !!skill);
  } catch (error) {
    console.error('[skillsCommand] Fuzzy match failed:', error);
    const lowerQuery = query.toLowerCase();
    return skills.filter((skill) =>
      skill.name.toLowerCase().startsWith(lowerQuery),
    );
  }
}
