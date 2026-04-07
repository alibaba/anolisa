"""Unit tests for security_middleware.router — action→backend routing."""

import unittest

from agent_sec_cli.security_middleware import router


class TestModuleToClassName(unittest.TestCase):
    def test_sandbox(self):
        name = router._module_to_class_name("agent_sec_cli.security_middleware.backends.sandbox")
        self.assertEqual(name, "SandboxBackend")

    def test_asset_verify(self):
        name = router._module_to_class_name("agent_sec_cli.security_middleware.backends.asset_verify")
        self.assertEqual(name, "AssetVerifyBackend")

    def test_hardening(self):
        name = router._module_to_class_name("agent_sec_cli.security_middleware.backends.hardening")
        self.assertEqual(name, "HardeningBackend")

    def test_single_word(self):
        name = router._module_to_class_name("some.module.foobar")
        self.assertEqual(name, "FoobarBackend")


class TestGetBackend(unittest.TestCase):
    def test_unknown_action_raises_value_error(self):
        with self.assertRaises(ValueError) as cm:
            router.get_backend("nonexistent_action")
        self.assertIn("nonexistent_action", str(cm.exception))

    def test_sandbox_prehook_returns_backend(self):
        backend = router.get_backend("sandbox_prehook")
        self.assertTrue(hasattr(backend, "execute"))

    def test_backend_is_cached(self):
        b1 = router.get_backend("sandbox_prehook")
        b2 = router.get_backend("sandbox_prehook")
        self.assertIs(b1, b2)


class TestRegisterAction(unittest.TestCase):
    def setUp(self):
        # Clean up test registrations after each test
        self._original_registry = dict(router._REGISTRY)
        self._original_cache = dict(router._backend_cache)

    def tearDown(self):
        router._REGISTRY.clear()
        router._REGISTRY.update(self._original_registry)
        router._backend_cache.clear()
        router._backend_cache.update(self._original_cache)

    def test_register_new_action(self):
        router.register_action("custom_test", "agent_sec_cli.security_middleware.backends.sandbox")
        backend = router.get_backend("custom_test")
        self.assertTrue(hasattr(backend, "execute"))

    def test_register_invalidates_cache(self):
        # Pre-cache sandbox_prehook
        b1 = router.get_backend("sandbox_prehook")
        # Re-register should invalidate cache
        router.register_action("sandbox_prehook", "agent_sec_cli.security_middleware.backends.sandbox")
        b2 = router.get_backend("sandbox_prehook")
        # After invalidation, a new instance is created
        self.assertIsNot(b1, b2)


if __name__ == "__main__":
    unittest.main()
