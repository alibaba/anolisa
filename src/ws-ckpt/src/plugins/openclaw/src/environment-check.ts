/**
 * Environment checker for the ws-ckpt plugin.
 *
 * Verifies that the runtime environment meets the requirements:
 * - ws-ckpt CLI binary is installed and on PATH
 * - ws-ckpt daemon is running (via `ws-ckpt status`)
 */

import { execFile } from "child_process";
import { promisify } from "util";

const execFileAsync = promisify(execFile);

/** Result of an environment check. */
export interface EnvironmentCheckResult {
  /** Whether all critical checks passed. */
  passed: boolean;
  /** Whether the ws-ckpt CLI binary is available. */
  cliAvailable: boolean;
  /** Whether the daemon is running. */
  daemonRunning: boolean;
  /** Critical errors that prevent plugin operation. */
  errors: string[];
  /** Non-critical warnings. */
  warnings: string[];
}

/**
 * Checks the runtime environment for ws-ckpt availability.
 *
 * The checker does not throw on failure — it returns a structured result
 * so the plugin can decide whether to operate in degraded mode.
 */
export class EnvironmentChecker {
  constructor() {
    // No config needed — checks are performed via CLI
  }

  /**
   * Run all environment checks and return a combined result.
   */
  public async check(): Promise<EnvironmentCheckResult> {
    const result: EnvironmentCheckResult = {
      passed: false,
      cliAvailable: false,
      daemonRunning: false,
      errors: [],
      warnings: [],
    };

    // 1. Check ws-ckpt CLI availability
    result.cliAvailable = await this.checkCli();
    if (!result.cliAvailable) {
      result.errors.push(
        "ws-ckpt CLI not found. Ensure ws-ckpt is installed and on PATH.",
      );
      // Cannot proceed with daemon health check without CLI
      return result;
    }

    // 2. Check daemon health via CLI
    result.daemonRunning = await this.checkDaemonHealth();

    if (!result.daemonRunning) {
      result.errors.push(
        "ws-ckpt daemon is not running. Ensure the daemon is running (systemctl status ws-ckpt).",
      );
    }

    // Overall pass requires CLI + daemon
    result.passed = result.cliAvailable && result.daemonRunning;

    return result;
  }

  /**
   * Generate a human-readable report from a check result.
   */
  public generateReport(result: EnvironmentCheckResult): string {
    const lines: string[] = [];

    lines.push("=== ws-ckpt Environment Check ===");
    lines.push("");
    lines.push(result.passed ? "Status: PASSED" : "Status: FAILED");
    lines.push("");
    lines.push(`  ws-ckpt CLI:    ${result.cliAvailable ? "OK" : "NOT FOUND"}`);
    lines.push(`  Daemon running:  ${result.daemonRunning ? "OK" : "NOT FOUND"}`);

    if (result.errors.length > 0) {
      lines.push("");
      lines.push("Errors:");
      for (const err of result.errors) {
        lines.push(`  - ${err}`);
      }
    }

    if (result.warnings.length > 0) {
      lines.push("");
      lines.push("Warnings:");
      for (const warn of result.warnings) {
        lines.push(`  - ${warn}`);
      }
    }

    return lines.join("\n");
  }

  /**
   * Check whether the `ws-ckpt` CLI binary is on PATH.
   */
  private async checkCli(): Promise<boolean> {
    try {
      await execFileAsync("which", ["ws-ckpt"], { timeout: 5000 });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Check daemon health via `ws-ckpt status`.
   *
   * - Exit code 0 → daemon running
   * - Non-zero → parse stdout/stderr to determine if daemon is down
   */
  private async checkDaemonHealth(): Promise<boolean> {
    try {
      await execFileAsync("ws-ckpt", ["status"], {
        timeout: 10000,
        encoding: "utf-8",
      });
      // Exit code 0 means daemon is running and healthy
      return true;
    } catch (err: unknown) {
      // Non-zero exit — try to parse output for details
      const error = err as { stdout?: string; stderr?: string };
      const stdout = error.stdout ?? "";
      const stderr = error.stderr ?? "";
      const combined = `${stdout}\n${stderr}`.toLowerCase();

      // If output mentions connection refused or socket, daemon is not running
      return (
        !combined.includes("connection refused") &&
        !combined.includes("not running") &&
        !combined.includes("could not connect") &&
        !combined.includes("no such file")
      );
    }
  }
}
