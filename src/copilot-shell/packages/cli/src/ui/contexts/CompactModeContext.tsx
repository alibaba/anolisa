/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { createContext, useContext, useMemo } from 'react';
import { useSettings } from './SettingsContext.js';
import { SettingScope } from '../../config/settings.js';

interface CompactModeContextType {
  verboseMode: boolean;
  setVerboseMode: (mode: boolean) => void;
  frozenSnapshot: unknown[] | null;
  setFrozenSnapshot: (snapshot: unknown[] | null) => void;
}

const CompactModeContext = createContext<CompactModeContextType | undefined>(
  undefined,
);

export const CompactModeProvider: React.FC<{
  children: React.ReactNode;
  value: { verboseMode: boolean; frozenSnapshot: unknown[] | null };
}> = ({
  children,
  value: {
    verboseMode: initialVerboseMode,
    frozenSnapshot: initialFrozenSnapshot,
  },
}) => {
  const { setValue } = useSettings();
  const [verboseMode, setVerboseModeState] = React.useState(initialVerboseMode);
  const [frozenSnapshot, setFrozenSnapshotState] = React.useState(
    initialFrozenSnapshot,
  );

  const setVerboseMode = React.useCallback(
    (mode: boolean) => {
      setVerboseModeState(mode);
      void setValue(SettingScope.User, 'ui.verboseMode', mode);
    },
    [setValue],
  );

  const value = useMemo(
    () => ({
      verboseMode,
      setVerboseMode,
      frozenSnapshot,
      setFrozenSnapshot: setFrozenSnapshotState,
    }),
    [verboseMode, setVerboseMode, frozenSnapshot, setFrozenSnapshotState],
  );

  return (
    <CompactModeContext.Provider value={value}>
      {children}
    </CompactModeContext.Provider>
  );
};

export const useCompactMode = (): CompactModeContextType => {
  const context = useContext(CompactModeContext);
  if (context === undefined) {
    throw new Error('useCompactMode must be used within a CompactModeProvider');
  }
  return context;
};
