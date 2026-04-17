/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type React from 'react';
import { Box, Text } from 'ink';
import { theme } from '../semantic-colors.js';
import { ConsoleSummaryDisplay } from './ConsoleSummaryDisplay.js';
import { ContextUsageDisplay } from './ContextUsageDisplay.js';
import { useTerminalSize } from '../hooks/useTerminalSize.js';
import { AutoAcceptIndicator } from './AutoAcceptIndicator.js';
import { ShellModeIndicator } from './ShellModeIndicator.js';

import { useStatusLine } from '../hooks/useStatusLine.js';
import { useUIState } from '../contexts/UIStateContext.js';
import { useConfig } from '../contexts/ConfigContext.js';
import { useVimMode } from '../contexts/VimModeContext.js';
import { useCompactMode } from '../contexts/CompactModeContext.js';
import { ApprovalMode } from '@copilot-shell/core';
import { t } from '../../i18n/index.js';

export const Footer: React.FC = () => {
  const uiState = useUIState();
  const config = useConfig();
  const { vimEnabled, vimMode } = useVimMode();
  const { verboseMode } = useCompactMode();
  const { text: statusLineText } = useStatusLine();

  const {
    errorCount,
    showErrorDetails,
    promptTokenCount,
    showAutoAcceptIndicator,
  } = {
    errorCount: uiState.errorCount,
    showErrorDetails: uiState.showErrorDetails,
    promptTokenCount: uiState.sessionStats.lastPromptTokenCount,
    showAutoAcceptIndicator: uiState.showAutoAcceptIndicator,
  };

  const showErrorIndicator = !showErrorDetails && errorCount > 0;

  const { columns: terminalWidth } = useTerminalSize();

  // Check if debug mode is enabled
  const debugMode = config.getDebugMode();

  const contextWindowSize =
    config.getContentGeneratorConfig()?.contextWindowSize;

  // Hide "? for shortcuts" when a custom status line is active (it already
  // occupies the top row, so the hint is redundant). Matches upstream behavior.
  const suppressHint = !!statusLineText;

  // Left section should show exactly ONE thing at any time, in priority order.
  const leftContent = uiState.ctrlCPressedOnce ? (
    <Text color={theme.status.warning}>{t('Press Ctrl+C again to exit.')}</Text>
  ) : uiState.ctrlDPressedOnce ? (
    <Text color={theme.status.warning}>{t('Press Ctrl+C again to exit.')}</Text>
  ) : uiState.showEscapePrompt ? (
    <Text color={theme.text.secondary}>{t('Press Esc again to clear.')}</Text>
  ) : vimEnabled && vimMode === 'INSERT' ? (
    <Text color={theme.text.secondary}>-- INSERT --</Text>
  ) : uiState.shellModeActive ? (
    <ShellModeIndicator />
  ) : showAutoAcceptIndicator !== undefined &&
    showAutoAcceptIndicator !== ApprovalMode.DEFAULT ? (
    <AutoAcceptIndicator approvalMode={showAutoAcceptIndicator} />
  ) : suppressHint ? null : (
    <Text color={theme.text.secondary}>{t('? for shortcuts')}</Text>
  );

  const rightItems: Array<{ key: string; node: React.ReactNode }> = [];
  if (debugMode) {
    rightItems.push({
      key: 'debug',
      node: <Text color={theme.status.warning}>Debug Mode</Text>,
    });
  }
  if (promptTokenCount > 0 && contextWindowSize) {
    rightItems.push({
      key: 'context',
      node: (
        <Text color={theme.text.accent}>
          <ContextUsageDisplay
            promptTokenCount={promptTokenCount}
            terminalWidth={terminalWidth}
            contextWindowSize={contextWindowSize}
          />
        </Text>
      ),
    });
  }
  if (showErrorIndicator) {
    rightItems.push({
      key: 'errors',
      node: <ConsoleSummaryDisplay errorCount={errorCount} />,
    });
  }

  if (verboseMode) {
    rightItems.push({
      key: 'verbose',
      node: <Text color={theme.text.accent}>{t('verbose')}</Text>,
    });
  }

  return (
    <Box
      justifyContent="space-between"
      width="100%"
      flexDirection="row"
      minHeight={1}
    >
      <Box flexShrink={1} width={Math.floor(terminalWidth * 0.6)}>
        {leftContent}
      </Box>
      <Box flexGrow={1} flexShrink={1} alignItems="flex-start">
        {statusLineText ? (
          <Text color={theme.text.secondary} wrap="truncate">
            {statusLineText}
          </Text>
        ) : null}
      </Box>
      <Box flexShrink={1} justifyContent="flex-end">
        <Box gap={1}>{rightItems.map((item) => item.node)}</Box>
      </Box>
    </Box>
  );
};
