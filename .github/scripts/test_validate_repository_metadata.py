#!/usr/bin/env python3
"""Regression tests for repository governance metadata validation."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, Callable

SOURCE_ROOT = Path(__file__).resolve().parents[2]


class RepositoryMetadataValidationTest(unittest.TestCase):
    """Run the validator against isolated metadata mutations."""

    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name) / "repository"
        shutil.copytree(SOURCE_ROOT / ".github", self.root / ".github")
        self.validator = self.root / ".github/scripts/validate-repository-metadata.py"

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    def run_validator(self) -> subprocess.CompletedProcess[str]:
        """Execute the copied validator and capture its diagnostics."""
        return subprocess.run(
            ["python3", str(self.validator)],
            cwd=self.root,
            check=False,
            capture_output=True,
            text=True,
        )

    def mutate_components(self, mutation: Callable[[dict[str, Any]], None]) -> None:
        """Apply one test mutation to the copied component metadata."""
        path = self.root / ".github/components.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        mutation(document)
        path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")

    def assert_validation_error(self, expected: str) -> str:
        """Require validation failure containing the expected diagnostic."""
        result = self.run_validator()
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn(expected, output)
        return output

    def test_committed_metadata_is_valid(self) -> None:
        result = self.run_validator()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_removed_component_leaves_no_silent_downstream_references(self) -> None:
        self.mutate_components(
            lambda document: document.__setitem__(
                "components",
                [component for component in document["components"] if component["id"] != "cosh"],
            )
        )
        output = self.assert_validation_error("unknown component scope 'cosh'")
        self.assertIn("unknown component option 'cosh'", output)
        self.assertIn("unknown component annotation component:cosh", output)
        self.assertIn("unknown component scope component:cosh", output)

    def test_question_form_must_contain_every_component_option(self) -> None:
        path = self.root / ".github/ISSUE_TEMPLATE/question.yml"
        path.write_text(
            path.read_text(encoding="utf-8").replace("        - sight\n", "", 1),
            encoding="utf-8",
        )
        self.assert_validation_error("question.yml: missing component option 'sight'")

    def test_issue_form_option_parsing_accepts_different_indentation(self) -> None:
        path = self.root / ".github/ISSUE_TEMPLATE/question.yml"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "        - sight\n", "          - sight\n", 1
            ),
            encoding="utf-8",
        )
        result = self.run_validator()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unknown_component_status_is_rejected(self) -> None:
        def change_status(document: dict[str, Any]) -> None:
            document["components"][0]["status"] = "actve"

        self.mutate_components(change_status)
        self.assert_validation_error("anolisa: unknown status 'actve'")

    def test_each_public_path_requires_its_codeowners_mapping(self) -> None:
        path = self.root / ".github/CODEOWNERS"
        lines = [
            line
            for line in path.read_text(encoding="utf-8").splitlines()
            if "/src/blaze/" not in line
        ]
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        self.assert_validation_error(
            "CODEOWNERS: /src/blaze/ does not map to component:blaze"
        )

    def test_undeclared_path_leaves_no_codeowners_mapping(self) -> None:
        path = self.root / ".github/CODEOWNERS"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "/src/legacy-blaze/ @casparant # auto-label: component:blaze\n",
            encoding="utf-8",
        )
        self.assert_validation_error(
            "CODEOWNERS: /src/legacy-blaze/ is not declared by blaze path prefixes"
        )

    def test_complete_codeowners_annotation_is_validated(self) -> None:
        path = self.root / ".github/CODEOWNERS"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "# auto-label: component:cosh",
                "# auto-label: component:cosh!",
                1,
            ),
            encoding="utf-8",
        )
        self.assert_validation_error(
            "CODEOWNERS: unknown component annotation component:cosh!"
        )

    def test_issue_options_must_be_unique(self) -> None:
        def duplicate_issue_option(document: dict[str, Any]) -> None:
            document["components"][1]["issue_option"] = document["components"][0][
                "issue_option"
            ]

        self.mutate_components(duplicate_issue_option)
        self.assert_validation_error("duplicate issue option: anolisa")

    def test_component_aliases_must_not_shadow_component_ids(self) -> None:
        def shadow_component_id(document: dict[str, Any]) -> None:
            document["components"][1]["aliases"].append("anolisa")

        self.mutate_components(shadow_component_id)
        self.assert_validation_error("duplicate component identifier: anolisa")

    def test_component_scopes_must_not_use_reserved_scopes(self) -> None:
        def use_reserved_scope(document: dict[str, Any]) -> None:
            document["components"][1]["commit_scope_aliases"].append("docs")

        self.mutate_components(use_reserved_scope)
        self.assert_validation_error(
            "component commit scope conflicts with reserved scope: 'docs'"
        )

    def test_issue_options_must_not_use_fallback_options(self) -> None:
        def use_fallback_option(document: dict[str, Any]) -> None:
            document["components"][1]["issue_option"] = "other"

        self.mutate_components(use_fallback_option)
        for form_name in ("bug_report.yml", "feature_request.yml", "question.yml"):
            path = self.root / ".github/ISSUE_TEMPLATE" / form_name
            path.write_text(
                path.read_text(encoding="utf-8").replace("        - cosh\n", "", 1),
                encoding="utf-8",
            )
        self.assert_validation_error(
            "component issue option conflicts with fallback option: 'other'"
        )

    def test_commit_scopes_and_aliases_must_be_globally_unique(self) -> None:
        def duplicate_commit_scope(document: dict[str, Any]) -> None:
            document["components"][1]["commit_scope_aliases"].append(
                document["components"][0]["commit_scope"]
            )

        self.mutate_components(duplicate_commit_scope)
        self.assert_validation_error("duplicate commit scope: anolisa")

    def test_ci_keys_must_be_unique(self) -> None:
        def duplicate_ci_key(document: dict[str, Any]) -> None:
            document["components"][1]["ci_key"] = document["components"][0]["ci_key"]

        self.mutate_components(duplicate_ci_key)
        self.assert_validation_error("duplicate CI key: anolisa")

    def test_ci_keys_must_match_the_workflow_contract(self) -> None:
        def change_ci_key(document: dict[str, Any]) -> None:
            document["components"][0]["ci_key"] = "not_a_real_ci_output"

        self.mutate_components(change_ci_key)
        self.assert_validation_error(
            "ci.yaml: missing declared output for component CI key 'not_a_real_ci_output'"
        )

    def test_ci_keys_must_route_to_their_components(self) -> None:
        def swap_ci_keys(document: dict[str, Any]) -> None:
            first, second = document["components"][:2]
            first["ci_key"], second["ci_key"] = second["ci_key"], first["ci_key"]

        self.mutate_components(swap_ci_keys)
        self.assert_validation_error(
            "ci.yaml: anolisa CI key 'copilot_shell' routes through ['anolisa']"
        )

    def test_ci_consumers_in_comments_are_ignored(self) -> None:
        path = self.root / ".github/workflows/ci.yaml"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "    if: needs.detect-changes.outputs.anolisa == 'true'",
                "    # if: needs.detect-changes.outputs.anolisa == 'true'\n"
                "    if: false",
                1,
            ),
            encoding="utf-8",
        )
        self.assert_validation_error(
            "ci.yaml: missing consumed output for component CI key 'anolisa'"
        )

    def test_ci_path_routes_in_comments_are_ignored(self) -> None:
        path = self.root / ".github/workflows/ci.yaml"
        old_route = (
            '          if echo "$CHANGED" | grep -q "^src/anolisa/"; then\n'
            "            ANOLISA=true\n"
            "          fi"
        )
        commented_route = (
            '          # if echo "$CHANGED" | grep -q "^src/anolisa/"; '
            "then ANOLISA=true"
        )
        path.write_text(
            path.read_text(encoding="utf-8").replace(old_route, commented_route, 1),
            encoding="utf-8",
        )
        self.assert_validation_error(
            "ci.yaml: anolisa path 'src/anolisa/' has no change route"
        )

    def test_ci_routes_reject_undeclared_path_alternatives(self) -> None:
        path = self.root / ".github/workflows/ci.yaml"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                'grep -q "^src/blaze/"',
                'grep -qE "^src/(legacy-blaze|blaze)/"',
                1,
            ),
            encoding="utf-8",
        )
        self.assert_validation_error(
            "ci.yaml: route pattern '^src/(legacy-blaze|blaze)/' "
            "is not an exact declared component path"
        )

    def test_release_prefixes_must_match_the_workflow_contract(self) -> None:
        def change_release_prefix(document: dict[str, Any]) -> None:
            document["components"][0]["release_tag_prefix"] = "not-a-real-prefix/v"

        self.mutate_components(change_release_prefix)
        self.assert_validation_error(
            "release.yaml: missing trigger prefix for component release prefix "
            "'not-a-real-prefix/v'"
        )

    def test_release_prefixes_must_route_to_their_components(self) -> None:
        def swap_release_prefixes(document: dict[str, Any]) -> None:
            first, second = document["components"][:2]
            first["release_tag_prefix"], second["release_tag_prefix"] = (
                second["release_tag_prefix"],
                first["release_tag_prefix"],
            )

        self.mutate_components(swap_release_prefixes)
        self.assert_validation_error(
            "release.yaml: anolisa prefix 'cosh/v' parses as 'copilot-shell'"
        )

    def test_public_component_requires_specific_or_default_triagers(self) -> None:
        def remove_fallback(document: dict[str, Any]) -> None:
            document["defaults"]["issue_triagers"] = []

        self.mutate_components(remove_fallback)
        self.assert_validation_error("ktuner: public components need an effective issue triager")


if __name__ == "__main__":
    unittest.main()
