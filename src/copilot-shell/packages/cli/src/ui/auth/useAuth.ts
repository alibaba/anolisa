/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type {
  Config,
  ContentGeneratorConfig,
  ModelProvidersConfig,
  AliyunCredentialsExtended,
} from '@copilot-shell/core';
import {
  AuthEvent,
  AuthType,
  encryptCredential,
  getErrorMessage,
  logAuth,
  saveAliyunCredentials,
} from '@copilot-shell/core';
import { useCallback, useEffect, useState } from 'react';
import type { LoadedSettings } from '../../config/settings.js';
import { getPersistScopeForModelSelection } from '../../config/modelProvidersScope.js';
import type { OpenAICredentials } from '../components/OpenAIKeyPrompt.js';
import { useQwenAuth } from '../hooks/useQwenAuth.js';
import { AuthState, MessageType } from '../types.js';
import type { HistoryItem } from '../types.js';
import { t } from '../../i18n/index.js';

export type { QwenAuthState } from '../hooks/useQwenAuth.js';

export const useAuthCommand = (
  settings: LoadedSettings,
  config: Config,
  addItem: (item: Omit<HistoryItem, 'id'>, timestamp: number) => void,
) => {
  const unAuthenticated = config.getAuthType() === undefined;

  const [authState, setAuthState] = useState<AuthState>(
    unAuthenticated ? AuthState.Updating : AuthState.Unauthenticated,
  );

  const [authError, setAuthError] = useState<string | null>(null);

  const [isAuthenticating, setIsAuthenticating] = useState(false);
  const [isAuthDialogOpen, setIsAuthDialogOpen] = useState(unAuthenticated);
  const [pendingAuthType, setPendingAuthType] = useState<AuthType | undefined>(
    undefined,
  );

  const { qwenAuthState, cancelQwenAuth } = useQwenAuth(
    pendingAuthType,
    isAuthenticating,
  );

  const onAuthError = useCallback(
    (error: string | null) => {
      setAuthError(error);
      if (error) {
        setAuthState(AuthState.Updating);
        setIsAuthDialogOpen(true);
      }
    },
    [setAuthError, setAuthState],
  );

  const handleAuthFailure = useCallback(
    (error: unknown) => {
      setIsAuthenticating(false);
      setIsAuthDialogOpen(true); // Reopen dialog to show error

      const errorMessage = t('Failed to authenticate. Message: {{message}}', {
        message: getErrorMessage(error),
      });

      setAuthError(errorMessage); // Set the error state
      onAuthError(errorMessage); // Also call the external error handler

      // Log authentication failure
      if (pendingAuthType) {
        const authEvent = new AuthEvent(
          pendingAuthType,
          'manual',
          'error',
          errorMessage,
        );
        logAuth(config, authEvent);
      }
    },
    [onAuthError, pendingAuthType, config],
  );

  const handleAuthSuccess = useCallback(
    async (authType: AuthType, credentials?: OpenAICredentials) => {
      try {
        const authTypeScope = getPersistScopeForModelSelection(settings);

        // Persist authType
        settings.setValue(
          authTypeScope,
          'security.auth.selectedType',
          authType,
        );

        // Persist model from ContentGenerator config (handles fallback cases)
        // This ensures that when syncAfterAuthRefresh falls back to default model,
        // it gets persisted to settings.json
        const contentGeneratorConfig = config.getContentGeneratorConfig();
        if (contentGeneratorConfig?.model) {
          settings.setValue(
            authTypeScope,
            'model.name',
            contentGeneratorConfig.model,
          );
          // 同时写入按认证方式隔离的模型字段，避免不同认证方式互相覆盖
          if (authType === AuthType.USE_OPENAI) {
            settings.setValue(
              authTypeScope,
              'security.auth.openaiModel',
              contentGeneratorConfig.model,
            );
          } else if (authType === AuthType.USE_ALIYUN) {
            settings.setValue(
              authTypeScope,
              'security.auth.aliyunModel',
              contentGeneratorConfig.model,
            );
          }
        } else if (authType === AuthType.USE_ALIYUN && credentials?.model) {
          // For Aliyun auth, specifically preserve the user-entered model
          settings.setValue(
            authTypeScope,
            'model.name',
            credentials.model.trim(),
          );
          settings.setValue(
            authTypeScope,
            'security.auth.aliyunModel',
            credentials.model.trim(),
          );
        }

        // Only update credentials if not switching to QWEN_OAUTH,
        // so that OpenAI credentials are preserved when switching to QWEN_OAUTH.
        if (authType !== AuthType.QWEN_OAUTH && credentials) {
          if (credentials?.apiKey != null) {
            settings.setValue(
              authTypeScope,
              'security.auth.apiKey',
              encryptCredential(credentials.apiKey),
            );
          }
          if (credentials?.baseUrl != null) {
            settings.setValue(
              authTypeScope,
              'security.auth.baseUrl',
              credentials.baseUrl,
            );
          }
        }
      } catch (error) {
        handleAuthFailure(error);
        return;
      }

      setAuthError(null);
      setAuthState(AuthState.Authenticated);
      setPendingAuthType(undefined);
      setIsAuthDialogOpen(false);
      setIsAuthenticating(false);

      // Log authentication success
      const authEvent = new AuthEvent(authType, 'manual', 'success');
      logAuth(config, authEvent);

      // Show success message
      addItem(
        {
          type: MessageType.INFO,
          text: t('Authenticated successfully with {{authType}} credentials.', {
            authType,
          }),
        },
        Date.now(),
      );
    },
    [settings, handleAuthFailure, config, addItem],
  );

  const performAuth = useCallback(
    async (authType: AuthType, credentials?: OpenAICredentials) => {
      try {
        await config.refreshAuth(authType);
        handleAuthSuccess(authType, credentials);
      } catch (e) {
        handleAuthFailure(e);
      }
    },
    [config, handleAuthSuccess, handleAuthFailure],
  );

  const isProviderManagedModel = useCallback(
    (authType: AuthType, modelId: string | undefined) => {
      if (!modelId) {
        return false;
      }

      const modelProviders = settings.merged.modelProviders as
        | ModelProvidersConfig
        | undefined;
      if (!modelProviders) {
        return false;
      }
      const providerModels = modelProviders[authType];
      if (!Array.isArray(providerModels)) {
        return false;
      }
      return providerModels.some(
        (providerModel) => providerModel.id === modelId,
      );
    },
    [settings],
  );

  const handleAuthSelect = useCallback(
    async (
      authType: AuthType | undefined,
      credentials?: OpenAICredentials | AliyunCredentialsExtended,
    ) => {
      if (!authType) {
        setIsAuthDialogOpen(false);
        setAuthError(null);
        return;
      }

      if (
        authType === AuthType.USE_OPENAI &&
        credentials &&
        'model' in credentials &&
        credentials.model &&
        isProviderManagedModel(authType, credentials.model)
      ) {
        onAuthError(
          t(
            'Model "{{modelName}}" is managed via settings.modelProviders. Please complete the fields in settings, or use another model id.',
            { modelName: credentials.model },
          ),
        );
        return;
      }

      setPendingAuthType(authType);
      setAuthError(null);
      setIsAuthDialogOpen(false);
      setIsAuthenticating(true);

      if (authType === AuthType.USE_OPENAI) {
        if (credentials && 'apiKey' in credentials) {
          // Pass settings.model.generationConfig to updateCredentials so it can be merged
          // after clearing provider-sourced config. This ensures settings.json generationConfig
          // fields (e.g., samplingParams, timeout) are preserved.
          const settingsGenerationConfig = settings.merged.model
            ?.generationConfig as Partial<ContentGeneratorConfig> | undefined;
          config.updateCredentials(
            {
              apiKey: credentials.apiKey,
              baseUrl: credentials.baseUrl,
              model: credentials.model,
            },
            settingsGenerationConfig,
          );
          await performAuth(authType, credentials as OpenAICredentials);
        }
        return;
      }

      if (authType === AuthType.USE_ALIYUN) {
        if (credentials && 'accessKeyId' in credentials) {
          try {
            // First validate the credentials structure
            if (
              !credentials.accessKeyId?.trim() ||
              !credentials.accessKeySecret?.trim()
            ) {
              throw new Error('Access Key ID and Secret cannot be empty');
            }

            // Save credentials to ~/.copilot-shell/aliyun_creds.json
            // 支持 STS 凭证（ECS RAM Role）和普通 AK/SK 凭证
            if (credentials.securityToken && credentials.expiration) {
              await saveAliyunCredentials({
                accessKeyId: credentials.accessKeyId.trim(),
                accessKeySecret: credentials.accessKeySecret.trim(),
                securityToken: credentials.securityToken,
                expiration: credentials.expiration,
              });
            } else {
              await saveAliyunCredentials({
                accessKeyId: credentials.accessKeyId.trim(),
                accessKeySecret: credentials.accessKeySecret.trim(),
              });
            }

            // Mark this as manual credentials so syncAfterAuthRefresh preserves the model
            // This is crucial for Aliyun auth to work properly
            const modelsConfig = config.getModelsConfig();
            if (modelsConfig) {
              if (typeof modelsConfig.updateCredentials === 'function') {
                modelsConfig.updateCredentials({
                  model: credentials.model?.trim(),
                });
              }
            }

            // Save model to settings.json for Header display
            if (credentials.model?.trim()) {
              const authTypeScope = getPersistScopeForModelSelection(settings);
              settings.setValue(
                authTypeScope,
                'model.name',
                credentials.model.trim(),
              );
              // 同时写入 aliyun 专属模型字段，避免不同认证方式互相覆盖
              settings.setValue(
                authTypeScope,
                'security.auth.aliyunModel',
                credentials.model.trim(),
              );
            }

            // Proceed with authentication
            await performAuth(authType);
          } catch (error) {
            // Ensure we show the error to the user and reset dialog state
            setIsAuthenticating(false);
            setIsAuthDialogOpen(true);
            handleAuthFailure(error);
          }
        }
        return;
      }

      await performAuth(authType);
    },
    [
      config,
      performAuth,
      handleAuthFailure,
      isProviderManagedModel,
      onAuthError,
      settings,
    ],
  );

  const openAuthDialog = useCallback(() => {
    setIsAuthDialogOpen(true);
  }, []);

  const cancelAuthentication = useCallback(() => {
    if (isAuthenticating && pendingAuthType === AuthType.QWEN_OAUTH) {
      cancelQwenAuth();
    }

    // Log authentication cancellation
    if (isAuthenticating && pendingAuthType) {
      const authEvent = new AuthEvent(pendingAuthType, 'manual', 'cancelled');
      logAuth(config, authEvent);
    }

    // Do not reset pendingAuthType here, persist the previously selected type.
    setIsAuthenticating(false);
    setIsAuthDialogOpen(true);
    setAuthError(null);
  }, [isAuthenticating, pendingAuthType, cancelQwenAuth, config]);

  /**
   /**
    * We previously used a useEffect to trigger authentication automatically when
    * settings.security.auth.selectedType changed. This caused problems: if authentication failed,
    * the UI could get stuck, since settings.json would update before success. Now, we
    * update selectedType in settings only when authentication fully succeeds.
    * Authentication is triggered explicitly—either during initial app startup or when the
    * user switches methods—not reactively through settings changes. This avoids repeated
    * or broken authentication cycles.
    */
  useEffect(() => {
    const defaultAuthType = process.env['QWEN_DEFAULT_AUTH_TYPE'];
    if (
      defaultAuthType &&
      ![
        AuthType.QWEN_OAUTH,
        AuthType.USE_OPENAI,
        AuthType.USE_ANTHROPIC,
        AuthType.USE_GEMINI,
        AuthType.USE_VERTEX_AI,
      ].includes(defaultAuthType as AuthType)
    ) {
      onAuthError(
        t(
          'Invalid QWEN_DEFAULT_AUTH_TYPE value: "{{value}}". Valid values are: {{validValues}}',
          {
            value: defaultAuthType,
            validValues: [
              AuthType.QWEN_OAUTH,
              AuthType.USE_OPENAI,
              AuthType.USE_ANTHROPIC,
              AuthType.USE_GEMINI,
              AuthType.USE_VERTEX_AI,
            ].join(', '),
          },
        ),
      );
    }
  }, [onAuthError]);

  return {
    authState,
    setAuthState,
    authError,
    onAuthError,
    isAuthDialogOpen,
    isAuthenticating,
    pendingAuthType,
    qwenAuthState,
    handleAuthSelect,
    openAuthDialog,
    cancelAuthentication,
  };
};
