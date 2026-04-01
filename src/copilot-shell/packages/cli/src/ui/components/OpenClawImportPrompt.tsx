/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import type React from 'react';
import { useCallback } from 'react';
import { Box, Text } from 'ink';
import { Colors } from '../colors.js';
import { useKeypress } from '../hooks/useKeypress.js';
import { t } from '../../i18n/index.js';
import type { OpenClawConfig } from '../../utils/openclawConfig.js';

interface OpenClawImportPromptProps {
  openclawConfig: OpenClawConfig;
  onAccept: (config: OpenClawConfig) => void;
  onDecline: () => void;
}

function maskApiKey(key: string): string {
  if (key.length <= 3) return '*'.repeat(key.length);
  return key.slice(0, 3) + '*'.repeat(Math.min(key.length - 3, 20));
}

export function OpenClawImportPrompt({
  openclawConfig,
  onAccept,
  onDecline,
}: OpenClawImportPromptProps): React.JSX.Element {
  const handleKeypress = useCallback(
    (key: { name?: string; sequence?: string }) => {
      const seq = key.sequence?.toLowerCase();
      if (seq === 'y') {
        onAccept(openclawConfig);
        return;
      }
      if (seq === 'n' || key.name === 'escape') {
        onDecline();
        return;
      }
    },
    [openclawConfig, onAccept, onDecline],
  );

  useKeypress(handleKeypress, { isActive: true });

  return (
    <Box
      borderStyle="round"
      borderColor={Colors.AccentBlue}
      flexDirection="column"
      padding={1}
      width="100%"
    >
      <Text bold color={Colors.AccentBlue}>
        {t('OpenClaw configuration detected')}
      </Text>
      <Box marginTop={1}>
        <Text>
          {t('The following configuration from OpenClaw will be imported')}:
        </Text>
      </Box>
      <Box marginTop={1} flexDirection="column" paddingLeft={2}>
        <Box flexDirection="row">
          <Box width={16}>
            <Text color={Colors.Gray}>{t('Provider:')}</Text>
          </Box>
          <Text>{openclawConfig.providerName}</Text>
        </Box>
        <Box flexDirection="row">
          <Box width={16}>
            <Text color={Colors.Gray}>{t('API Key:')}</Text>
          </Box>
          <Text>{maskApiKey(openclawConfig.apiKey)}</Text>
        </Box>
        <Box flexDirection="row">
          <Box width={16}>
            <Text color={Colors.Gray}>{t('Base URL:')}</Text>
          </Box>
          <Text>{openclawConfig.baseUrl}</Text>
        </Box>
        {openclawConfig.model ? (
          <Box flexDirection="row">
            <Box width={16}>
              <Text color={Colors.Gray}>{t('Model:')}</Text>
            </Box>
            <Text>{openclawConfig.model}</Text>
          </Box>
        ) : null}
      </Box>
      <Box marginTop={1}>
        <Text color={Colors.Gray}>
          {t('Press Y to authorize import, N to configure manually')}
        </Text>
      </Box>
    </Box>
  );
}
