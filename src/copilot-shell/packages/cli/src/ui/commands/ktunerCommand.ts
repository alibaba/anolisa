/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type { SlashCommand, MessageActionReturn } from './types.js';
import { CommandKind } from './types.js';
import { t } from '../../i18n/index.js';
import { SettingScope } from '../../config/settings.js';
import { maybeRunKtunerFirstRunCheck } from '../../utils/ktunerFirstRun.js';

const enableSubCommand: SlashCommand = {
  name: 'enable',
  get description() {
    return t('Enable the read-only ktuner kernel tuning check');
  },
  kind: CommandKind.BUILT_IN,
  action: async (context): Promise<MessageActionReturn> => {
    await context.services.settings.setValue(
      SettingScope.User,
      'general.ktunerCheck',
      'enabled',
    );
    void maybeRunKtunerFirstRunCheck(context.ui.addItem);
    return {
      type: 'message',
      messageType: 'info',
      content: t(
        'ktuner check enabled. It runs a read-only kernel scan after auth and never changes anything.',
      ),
    };
  },
};

const disableSubCommand: SlashCommand = {
  name: 'disable',
  get description() {
    return t('Stop the ktuner kernel tuning check and hint');
  },
  kind: CommandKind.BUILT_IN,
  action: async (context): Promise<MessageActionReturn> => {
    await context.services.settings.setValue(
      SettingScope.User,
      'general.ktunerCheck',
      'disabled',
    );
    return {
      type: 'message',
      messageType: 'info',
      content: t(
        'ktuner check disabled. Re-enable it anytime with /ktuner enable.',
      ),
    };
  },
};

export const ktunerCommand: SlashCommand = {
  name: 'ktuner',
  get description() {
    return t('Control the read-only ktuner kernel tuning check');
  },
  kind: CommandKind.BUILT_IN,
  subCommands: [enableSubCommand, disableSubCommand],
  action: (context): MessageActionReturn => {
    const mode =
      (context.services.settings.merged.general?.ktunerCheck as
        | string
        | undefined) ?? 'ask';
    return {
      type: 'message',
      messageType: 'info',
      content: t(
        'ktuner check is currently "{{mode}}". Use /ktuner enable or /ktuner disable to change it.',
        { mode },
      ),
    };
  },
};
