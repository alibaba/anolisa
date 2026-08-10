const assert = require('node:assert/strict');
const test = require('node:test');

const { resolveLocale } = require(process.env.AGENTSIGHT_I18N_BUILD);

test('resolveLocale selects the first supported browser locale', () => {
  assert.equal(resolveLocale(null, ['fr-FR', 'zh-CN']), 'zh-CN');
  assert.equal(resolveLocale(null, ['fr-FR', 'en-GB']), 'en-US');
  assert.equal(resolveLocale(null, ['zh-HK', 'en-US']), 'zh-CN');
  assert.equal(resolveLocale(null, ['de-DE', 'fr-FR']), 'en-US');
});

test('resolveLocale keeps a supported persisted locale as the highest priority', () => {
  assert.equal(resolveLocale('zh-CN', ['en-US']), 'zh-CN');
  assert.equal(resolveLocale('en-US', ['zh-CN']), 'en-US');
});

test('resolveLocale rejects an unsupported persisted locale', () => {
  assert.equal(resolveLocale('fr-FR', ['en-US']), 'en-US');
});

test('resolveLocale skips falsy browser languages', () => {
  assert.equal(resolveLocale(null, [undefined, 'zh-CN']), 'zh-CN');
  assert.equal(resolveLocale(null, [undefined]), 'en-US');
});
