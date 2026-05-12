/**
 * Command executor for the ws-ckpt plugin.
 *
 * Wraps all `ws-ckpt` CLI invocations using `child_process.execFile`.
 * Each method constructs the appropriate CLI arguments, executes the
 * command, and returns a typed {@link CommandOutput}.
 */

import { execFile } from "child_process";
import { promisify } from "util";
import type { CommandOutput } from "./types.js";

const execFileAsync = promisify(execFile);

/** Default command execution timeout in milliseconds. */
const DEFAULT_TIMEOUT_MS = 30_000;

/** The ws-ckpt CLI binary name. */
const WS_CKPT_BIN = "ws-ckpt";

/**
 * Executes ws-ckpt CLI commands and returns structured output.
 *
 * All methods are async and resolve with a {@link CommandOutput} containing
 * the exit code, stdout, and stderr of the CLI invocation.
 */
export class CommandExecutor {
  private timeoutMs: number;

  /**
   * Create a new CommandExecutor.
   *
   * @param timeoutMs - Timeout for each CLI invocation (default 30 s).
   */
  constructor(timeoutMs: number = DEFAULT_TIMEOUT_MS) {
    this.timeoutMs = timeoutMs;
  }

  // -----------------------------------------------------------------------
  // Phase 1 commands
  // -----------------------------------------------------------------------

  /**
   * Initialize a workspace for ws-ckpt management.
   *
   * Equivalent to: `ws-ckpt init --workspace <workspace>`
   *
   * @param workspace - Workspace directory path.
   */
  public async init(workspace: string): Promise<CommandOutput> {
    return this.run(["init", "--workspace", workspace]);
  }

  /**
   * Create a checkpoint (snapshot) of the workspace.
   *
   * Equivalent to:
   * ```
   * ws-ckpt checkpoint --workspace <ws> --id <id> [--message <msg>] [--metadata <json>]
   * ```
   *
   * @param workspace - Workspace directory path.
   * @param id        - Caller-provided snapshot identifier.
   * @param options   - Optional message and metadata.
   */
  public async checkpoint(
    workspace: string,
    id: string,
    options?: { message?: string; metadata?: string },
  ): Promise<CommandOutput> {
    const args = [
      "checkpoint",
      "--workspace", workspace,
      "--id", id,
    ];

    if (options?.message) {
      args.push("--message", options.message);
    }

    if (options?.metadata) {
      args.push("--metadata", options.metadata);
    }

    return this.run(args);
  }

  /**
   * Roll back the workspace to a specific snapshot.
   *
   * Equivalent to: `ws-ckpt rollback --workspace <ws> --snapshot <target>`
   *
   * @param workspace - Workspace directory path.
   * @param target    - Snapshot identifier or name to roll back to.
   */
  public async rollback(workspace: string, target: string): Promise<CommandOutput> {
    return this.run(["rollback", "--workspace", workspace, "--snapshot", target]);
  }

  /**
   * Delete a specific snapshot.
   *
   * Equivalent to: `ws-ckpt delete [--workspace <ws>] --snapshot <id> [--force]`
   *
   * @param snapshot  - Snapshot ID to delete.
   * @param options   - Optional workspace and force flag.
   */
  public async delete(
    snapshot: string,
    options?: { workspace?: string; force?: boolean },
  ): Promise<CommandOutput> {
    const args = ["delete"];

    if (options?.workspace) {
      args.push("--workspace", options.workspace);
    }

    args.push("--snapshot", snapshot);

    if (options?.force) {
      args.push("--force");
    }

    return this.run(args);
  }

  // -----------------------------------------------------------------------
  // Phase 2 commands
  // -----------------------------------------------------------------------

  /**
   * List all snapshots for a workspace.
   *
   * Equivalent to: `ws-ckpt list --workspace <ws> [--format <fmt>]`
   *
   * @param workspace - Workspace directory path.
   * @param format    - Output format: "table" or "json" (default "json").
   */
  public async list(workspace: string, format: "table" | "json" = "json"): Promise<CommandOutput> {
    return this.run(["list", "--workspace", workspace, "--format", format]);
  }

  /**
   * Show the diff between two snapshots.
   *
   * Equivalent to: `ws-ckpt diff --workspace <ws> --from <a> --to <b>`
   *
   * @param workspace - Workspace directory path.
   * @param from      - Source snapshot identifier or name.
   * @param to        - Target snapshot identifier or name.
   */
  public async diff(workspace: string, from: string, to: string): Promise<CommandOutput> {
    return this.run([
      "diff",
      "--workspace", workspace,
      "--from", from,
      "--to", to,
    ]);
  }

  /**
   * Query daemon and/or workspace status.
   *
   * Equivalent to: `ws-ckpt status [--workspace <ws>]`
   *
   * @param workspace - Optional workspace path for workspace-specific status.
   */
  public async status(workspace?: string): Promise<CommandOutput> {
    const args = ["status"];
    if (workspace) {
      args.push("--workspace", workspace);
    }
    return this.run(args);
  }

  /**
   * Clean up old snapshots, keeping the most recent N.
   *
   * Equivalent to: `ws-ckpt cleanup --workspace <ws> [--keep <N>]`
   *
   * @param workspace - Workspace directory path.
   * @param keep      - Number of snapshots to keep.
   */
  public async cleanup(workspace: string, keep?: number): Promise<CommandOutput> {
    const args = ["cleanup", "--workspace", workspace];
    if (keep !== undefined) {
      args.push("--keep", String(keep));
    }
    return this.run(args);
  }

  /**
   * View or update daemon config.
   * Maps to `ws-ckpt config [flags]`.
   */
  public async config(options?: {
    enableAutoCleanup?: boolean;
    disableAutoCleanup?: boolean;
    autoCleanupKeep?: string;
  }): Promise<CommandOutput> {
    const args = ["config"];
    if (options?.enableAutoCleanup) args.push("--enable-auto-cleanup");
    if (options?.disableAutoCleanup) args.push("--disable-auto-cleanup");
    if (options?.autoCleanupKeep !== undefined) args.push("--auto-cleanup-keep", options.autoCleanupKeep);
    return this.run(args);
  }

  // -----------------------------------------------------------------------
  // Internal
  // -----------------------------------------------------------------------

  /**
   * Execute a ws-ckpt CLI command and return structured output.
   *
   * @param args - CLI arguments (excluding the binary name).
   * @returns A {@link CommandOutput} with exit code, stdout, and stderr.
   */
  private async run(args: string[]): Promise<CommandOutput> {
    try {
      const { stdout, stderr } = await execFileAsync(WS_CKPT_BIN, args, {
        timeout: this.timeoutMs,
        encoding: "utf-8",
      });

      return {
        exitCode: 0,
        stdout: stdout ?? "",
        stderr: stderr ?? "",
      };
    } catch (error: unknown) {
      // execFile rejects with an error that may contain exit code and output.
      const err = error as {
        code?: number | string;
        stdout?: string;
        stderr?: string;
        message?: string;
      };

      return {
        exitCode: typeof err.code === "number" ? err.code : 1,
        stdout: err.stdout ?? "",
        stderr: err.stderr ?? err.message ?? "Unknown command error",
      };
    }
  }
}
