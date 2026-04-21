/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type { SlashCommand, MessageActionReturn } from './types.js';
import { CommandKind } from './types.js';
import { t } from '../../i18n/index.js';
import { SettingScope } from '../../config/settings.js';

type StatusLineActionReturn =
  | MessageActionReturn
  | {
      type: 'submit_prompt';
      content: Array<{ text: string }>;
    };

export const statuslineCommand: SlashCommand = {
  name: 'statusline',
  get description() {
    return t("Set up Copilot Shell's status line UI");
  },
  kind: CommandKind.BUILT_IN,
  action: async (context, args): Promise<StatusLineActionReturn> => {
    // Split the command and arguments
    const trimmedArgs = args.trim();

    // If no arguments, show current status or usage info
    if (!trimmedArgs) {
      // Show current status line configuration
      const currentConfig = context.services.settings.merged.ui?.statusLine as
        | { command: string }
        | undefined;
      if (currentConfig) {
        return {
          type: 'message',
          messageType: 'info',
          content: t('Current status line command: {{command}}', {
            command: currentConfig.command,
          }),
        };
      } else {
        return {
          type: 'message',
          messageType: 'info',
          content: t('No status line command is currently set.'),
        };
      }
    }

    // Check if the command is 'clear' or 'off' to remove the configuration
    if (
      trimmedArgs.toLowerCase() === 'clear' ||
      trimmedArgs.toLowerCase() === 'off'
    ) {
      await context.services.settings.setValue(
        SettingScope.User,
        'ui.statusLine',
        undefined,
      );
      return {
        type: 'message',
        messageType: 'info',
        content: t('Status line command cleared.'),
      };
    }

    // Otherwise, set the status line command
    const statusLineConfig = {
      type: 'command',
      command: trimmedArgs,
    };

    await context.services.settings.setValue(
      SettingScope.User,
      'ui.statusLine',
      statusLineConfig,
    );
    return {
      type: 'message',
      messageType: 'info',
      content: t('Status line command set to: {{command}}', {
        command: trimmedArgs,
      }),
    };
  },
};
