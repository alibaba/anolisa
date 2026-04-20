"""Unit tests for prompt_scanner.config (ScanConfig / ScanMode / get_config)."""

import unittest

from agent_sec_cli.prompt_scanner.config import (
    PRESET_CONFIGS,
    ScanConfig,
    ScanMode,
    get_config,
)


class TestScanMode(unittest.TestCase):
    def test_all_values(self) -> None:
        self.assertEqual(ScanMode.FAST.value, "fast")
        self.assertEqual(ScanMode.STANDARD.value, "standard")
        self.assertEqual(ScanMode.STRICT.value, "strict")

    def test_from_string(self) -> None:
        self.assertEqual(ScanMode("fast"), ScanMode.FAST)
        self.assertEqual(ScanMode("strict"), ScanMode.STRICT)

    def test_invalid_value_raises(self) -> None:
        with self.assertRaises(ValueError):
            ScanMode("turbo")


class TestScanConfig(unittest.TestCase):
    def test_defaults(self) -> None:
        cfg = ScanConfig()
        self.assertEqual(cfg.layers, ["rule_engine", "ml_classifier"])
        self.assertTrue(cfg.fast_fail)
        self.assertEqual(cfg.threshold, 0.5)
        self.assertIsNone(cfg.custom_rules_path)
        self.assertTrue(cfg.detect_encoding)

    def test_model_name_default_is_86m(self) -> None:
        cfg = ScanConfig()
        self.assertIn("86M", cfg.model_name)
        self.assertIn("LLM-Research", cfg.model_name)

    def test_custom_threshold(self) -> None:
        cfg = ScanConfig(threshold=0.3)
        self.assertEqual(cfg.threshold, 0.3)

    def test_custom_layers(self) -> None:
        cfg = ScanConfig(layers=["rule_engine"])
        self.assertEqual(cfg.layers, ["rule_engine"])

    def test_custom_rules_path(self) -> None:
        cfg = ScanConfig(custom_rules_path="/tmp/my_rules.yaml")
        self.assertEqual(cfg.custom_rules_path, "/tmp/my_rules.yaml")


class TestPresetConfigs(unittest.TestCase):
    def test_fast_preset_layers(self) -> None:
        self.assertEqual(PRESET_CONFIGS[ScanMode.FAST].layers, ["rule_engine"])

    def test_standard_preset_layers(self) -> None:
        self.assertIn("rule_engine", PRESET_CONFIGS[ScanMode.STANDARD].layers)
        self.assertIn("ml_classifier", PRESET_CONFIGS[ScanMode.STANDARD].layers)

    def test_strict_preset_layers(self) -> None:
        layers = PRESET_CONFIGS[ScanMode.STRICT].layers
        self.assertIn("rule_engine", layers)
        self.assertIn("ml_classifier", layers)

    def test_strict_fast_fail_is_false(self) -> None:
        self.assertFalse(PRESET_CONFIGS[ScanMode.STRICT].fast_fail)

    def test_fast_fast_fail_is_true(self) -> None:
        self.assertTrue(PRESET_CONFIGS[ScanMode.FAST].fast_fail)


class TestGetConfig(unittest.TestCase):
    def test_returns_copy(self) -> None:
        c1 = get_config(ScanMode.FAST)
        c2 = get_config(ScanMode.FAST)
        # Modifying one must not affect the other
        c1.threshold = 0.99
        c2_threshold = get_config(ScanMode.FAST).threshold
        self.assertNotEqual(c2_threshold, 0.99)

    def test_all_modes_return_config(self) -> None:
        for mode in ScanMode:
            cfg = get_config(mode)
            self.assertIsInstance(cfg, ScanConfig)

    def test_invalid_mode_raises(self) -> None:
        # Pass a string that is not a valid ScanMode to exercise the ValueError branch
        with self.assertRaises((ValueError, KeyError)):
            # Use an object that is not in PRESET_CONFIGS
            get_config("invalid_mode")  # type: ignore[arg-type]
