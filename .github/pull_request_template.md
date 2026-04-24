## Description

<!-- Provide a clear and concise description of the changes. Include motivation and context. -->

## Related Issue

<!--
  REQUIRED: Every PR must be linked to an existing issue.
  Use one of the closing keywords so the issue closes automatically on merge:

    closes #<number>
    fixes #<number>
    resolves #<number>

  If this is a trivial typo / doc-only fix with no issue, write:
    no-issue: <reason>
-->

closes #

## Type of Change

<!-- Check all that apply. -->

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactoring (no functional change)
- [ ] Performance improvement
- [ ] CI/CD or build changes

## Scope

<!-- Which sub-project does this PR affect? -->

- [ ] `cosh` (copilot-shell)
- [ ] `sec-core` (agent-sec-core)
- [ ] `skill` (os-skills)
- [ ] `sight` (agentsight)
- [ ] `tokenless` (tokenless)
- [ ] Multiple / Project-wide

## Checklist

<!-- Check all that apply. -->

- [ ] I have read the [Contributing Guide](../CONTRIBUTING.md)
- [ ] My code follows the project's code style
- [ ] I have added tests that prove my fix is effective or that my feature works
- [ ] I have updated the documentation accordingly
- [ ] For `cosh`: Lint passes, type check passes, and tests pass
- [ ] For `sec-core` (Rust): `cargo clippy -- -D warnings` and `cargo fmt --check` pass
- [ ] For `sec-core` (Python): Ruff format and pytest pass
- [ ] For `skill`: Skill directory structure is valid and shell scripts pass syntax check
- [ ] For `sight`: `cargo clippy -- -D warnings` and `cargo fmt --check` pass
- [ ] For `tokenless`: `cargo clippy -- -D warnings` and `cargo fmt --check` pass
- [ ] Lock files are up to date (`package-lock.json` / `Cargo.lock`)

## Testing

<!-- Describe the tests you ran and how to reproduce them. -->

## Additional Notes

<!-- Any additional information, screenshots, or context. -->
