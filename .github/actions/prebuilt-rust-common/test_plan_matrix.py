#!/usr/bin/env python3
"""Regression tests for the prebuilt release matrix planner."""

from __future__ import annotations

import unittest

import plan_matrix


class PlanMatrixTests(unittest.TestCase):
    def test_anolisa_targets(self) -> None:
        matrix = plan_matrix.build_matrix("anolisa")

        self.assertEqual(
            [(row["target-os"], row["target-arch"], row["profile"]) for row in matrix["include"]],
            [
                ("linux", "x86_64", "gnu2.17-x86_64"),
                ("linux", "aarch64", "gnu2.17-aarch64"),
                ("macos", "aarch64", "darwin11-aarch64"),
            ],
        )
        self.assertTrue(all(row["component"] == "anolisa" for row in matrix["include"]))

    def test_cosh_ng_targets(self) -> None:
        matrix = plan_matrix.build_matrix("cosh-ng")

        self.assertEqual(
            [(row["target-os"], row["target-arch"], row["profile"]) for row in matrix["include"]],
            [
                ("linux", "x86_64", "gnu2.28-x86_64"),
                ("linux", "aarch64", "gnu2.28-aarch64"),
                ("macos", "aarch64", "darwin11-aarch64"),
            ],
        )
        self.assertTrue(all(row["component"] == "cosh-ng" for row in matrix["include"]))

    def test_tokenless_targets(self) -> None:
        matrix = plan_matrix.build_matrix("tokenless")

        self.assertEqual(
            [(row["target-os"], row["target-arch"], row["profile"]) for row in matrix["include"]],
            [
                ("linux", "x86_64", "gnu2.17-x86_64"),
                ("linux", "aarch64", "gnu2.17-aarch64"),
                ("macos", "aarch64", "darwin11-aarch64"),
            ],
        )
        self.assertTrue(all(row["component"] == "tokenless" for row in matrix["include"]))

    def test_component_without_prebuilt_targets(self) -> None:
        self.assertEqual(plan_matrix.build_matrix("agentsight"), {"include": []})


if __name__ == "__main__":
    unittest.main()
