/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { promises as fs } from 'node:fs';
import {
  saveAliyunCredentials,
  loadAliyunCredentials,
  clearAliyunCredentials,
  hasAliyunCredentials,
  getAliyunCredsPath,
} from './aliyunCredentials.js';

vi.mock('node:fs', () => ({
  promises: {
    readFile: vi.fn(),
    writeFile: vi.fn(),
    mkdir: vi.fn(),
    unlink: vi.fn(),
  },
}));

vi.mock('../utils/credential-encryptor.js', () => ({
  encryptCredential: vi.fn((v: string) => `enc:mock:mock:${v}`),
  decryptCredential: vi.fn((v: string) => {
    if (v.startsWith('enc:mock:mock:')) {
      return v.slice('enc:mock:mock:'.length);
    }
    if (v.startsWith('enc:')) {
      // Simulate decryption failure for unknown encrypted values
      return undefined;
    }
    // Plaintext passthrough
    return v;
  }),
}));

const mockFs = fs as unknown as {
  readFile: ReturnType<typeof vi.fn>;
  writeFile: ReturnType<typeof vi.fn>;
  mkdir: ReturnType<typeof vi.fn>;
  unlink: ReturnType<typeof vi.fn>;
};

const testCredentials = {
  accessKeyId: 'LTAI5tTestKeyId',
  accessKeySecret: 'TestSecretValue123',
};

describe('aliyunCredentials', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockFs.mkdir.mockResolvedValue(undefined);
    mockFs.writeFile.mockResolvedValue(undefined);
  });

  describe('saveAliyunCredentials', () => {
    it('should write encrypted credentials to file', async () => {
      await saveAliyunCredentials(testCredentials);

      expect(mockFs.mkdir).toHaveBeenCalled();
      expect(mockFs.writeFile).toHaveBeenCalledWith(
        getAliyunCredsPath(),
        `enc:mock:mock:${JSON.stringify(testCredentials)}`,
        { mode: 0o600 },
      );
    });

    it('should throw on EACCES error', async () => {
      const eaccesError = Object.assign(new Error('Permission denied'), {
        code: 'EACCES',
      });
      mockFs.mkdir.mockRejectedValue(eaccesError);

      await expect(saveAliyunCredentials(testCredentials)).rejects.toThrow(
        'Permission denied (EACCES)',
      );
    });

    it('should throw generic error on other failures', async () => {
      mockFs.writeFile.mockRejectedValue(new Error('disk full'));

      await expect(saveAliyunCredentials(testCredentials)).rejects.toThrow(
        'disk full',
      );
    });
  });

  describe('loadAliyunCredentials', () => {
    it('should load and decrypt encrypted credentials', async () => {
      mockFs.readFile.mockResolvedValue(
        `enc:mock:mock:${JSON.stringify(testCredentials)}`,
      );

      const result = await loadAliyunCredentials();
      expect(result).toEqual(testCredentials);
    });

    it('should load plaintext JSON credentials (backward compat)', async () => {
      mockFs.readFile.mockResolvedValue(
        JSON.stringify(testCredentials, null, 2),
      );

      const result = await loadAliyunCredentials();
      expect(result).toEqual(testCredentials);
    });

    it('should return null when decryption fails', async () => {
      mockFs.readFile.mockResolvedValue('enc:unknown:bad:data');

      const result = await loadAliyunCredentials();
      expect(result).toBeNull();
    });

    it('should return null when file does not exist', async () => {
      mockFs.readFile.mockRejectedValue(
        Object.assign(new Error('ENOENT'), { code: 'ENOENT' }),
      );

      const result = await loadAliyunCredentials();
      expect(result).toBeNull();
    });

    it('should return null for invalid credentials format', async () => {
      mockFs.readFile.mockResolvedValue(
        JSON.stringify({ accessKeyId: '', accessKeySecret: '' }),
      );

      const result = await loadAliyunCredentials();
      expect(result).toBeNull();
    });

    it('should return null on read error', async () => {
      mockFs.readFile.mockRejectedValue(new Error('read error'));

      const result = await loadAliyunCredentials();
      expect(result).toBeNull();
    });
  });

  describe('clearAliyunCredentials', () => {
    it('should delete the credentials file', async () => {
      mockFs.unlink.mockResolvedValue(undefined);

      await clearAliyunCredentials();
      expect(mockFs.unlink).toHaveBeenCalledWith(getAliyunCredsPath());
    });

    it('should not throw when file does not exist', async () => {
      mockFs.unlink.mockRejectedValue(
        Object.assign(new Error('ENOENT'), { code: 'ENOENT' }),
      );

      await expect(clearAliyunCredentials()).resolves.not.toThrow();
    });
  });

  describe('hasAliyunCredentials', () => {
    it('should return true when credentials exist', async () => {
      mockFs.readFile.mockResolvedValue(
        `enc:mock:mock:${JSON.stringify(testCredentials)}`,
      );

      expect(await hasAliyunCredentials()).toBe(true);
    });

    it('should return false when no credentials', async () => {
      mockFs.readFile.mockRejectedValue(
        Object.assign(new Error('ENOENT'), { code: 'ENOENT' }),
      );

      expect(await hasAliyunCredentials()).toBe(false);
    });
  });
});
