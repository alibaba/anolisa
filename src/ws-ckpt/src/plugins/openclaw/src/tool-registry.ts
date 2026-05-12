/**
 * Tool registration for the ws-ckpt OpenClaw plugin.
 *
 * registerTools() registers all 7 ws-ckpt tools with the OpenClaw API.
 * Command registration (registerCommand) is intentionally omitted —
 * all capability is exposed exclusively via Tool Calling.
 */

import type { OpenClawPluginApi } from "../types-shim.js";
import {
  handleCheckpoint,
  handleRollback,
  handleListCheckpoints,
  handleDelete,
  handleDiff,
  handleStatus,
  handleConfig,
  textToolResult,
} from "./handlers.js";

/**
 * Register all 7 ws-ckpt tools with the OpenClaw plugin API.
 *
 * @param api - Plugin API provided by the OpenClaw runtime.
 */
export function registerTools(api: OpenClawPluginApi): void {
  // --- ws-ckpt-config ---
  api.registerTool(
    {
      name: "ws-ckpt-config",
      description: "View or update ws-ckpt plugin configuration. Only update the specific key explicitly requested by the user.",
      parameters: {
        type: "object",
        properties: {
          action: {
            type: "string",
            description:
              'Action to perform: "view" (default) or "update"',
          },
          key: {
            type: "string",
            description:
              "Config key to update (autoCheckpoint, maxSnapshotsNum, maxSnapshotsDuration)",
          },
          value: {
            type: "string",
            description: "New value for the config key. For maxSnapshotsNum/maxSnapshotsDuration, pass \"unset\" to clear the value and disable auto-cleanup when both are unset.",
          },
        },
      },
      async execute(_toolCallId, params) {
        const r = await handleConfig(
          params.action as string | undefined,
          params.key as string | undefined,
          params.value as string | undefined,
        );
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-config" },
  );

  // --- ws-ckpt-checkpoint ---
  api.registerTool(
    {
      name: "ws-ckpt-checkpoint",
      description: "Create a checkpoint of the current workspace. Communicates directly with ws-ckpt daemon — no additional CLI verification needed.",
      parameters: {
        type: "object",
        properties: {
          id: {
            type: "string",
            description: "Required: caller-provided snapshot identifier",
          },
          message: {
            type: "string",
            description: "Optional message describing the checkpoint",
          },
        },
        required: ["id"],
      },
      async execute(_toolCallId, params) {
        const r = await handleCheckpoint(JSON.stringify(params));
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-checkpoint" },
  );

  // --- ws-ckpt-rollback ---
  api.registerTool(
    {
      name: "ws-ckpt-rollback",
      description: "Roll back the workspace to a specific checkpoint. Communicates directly with ws-ckpt daemon — no additional CLI verification needed.",
      parameters: {
        type: "object",
        properties: {
          target: {
            type: "string",
            description:
              "Snapshot hash id to roll back to",
          },
        },
        required: ["target"],
      },
      async execute(_toolCallId, params) {
        const r = await handleRollback(params.target as string | undefined);
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-rollback" },
  );

  // --- ws-ckpt-list ---
  api.registerTool(
    {
      name: "ws-ckpt-list",
      description: "List all checkpoints managed by ws-ckpt. Always display the FULL untruncated result to the user.",
      parameters: { type: "object", properties: {} },
      async execute() {
        const r = await handleListCheckpoints();
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-list" },
  );

  // --- ws-ckpt-diff ---
  api.registerTool(
    {
      name: "ws-ckpt-diff",
      description: "Compare file changes between two checkpoints. Always display the FULL untruncated result to the user. Do NOT re-interpret or contradict the tool output.",
      parameters: {
        type: "object",
        properties: {
          from: {
            type: "string",
            description: "Source snapshot id or name",
          },
          to: {
            type: "string",
            description:
              "Target snapshot id or name (defaults to current state)",
          },
        },
        required: ["from", "to"],
      },
      async execute(_toolCallId, params) {
        const r = await handleDiff(
          params.from as string | undefined,
          params.to as string | undefined,
        );
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-diff" },
  );

  // --- ws-ckpt-delete ---
  api.registerTool(
    {
      name: "ws-ckpt-delete",
      description: "Delete a specific snapshot. Communicates directly with ws-ckpt daemon — no additional CLI verification needed.",
      parameters: {
        type: "object",
        properties: {
          snapshot: {
            type: "string",
            description: "Required: snapshot ID to delete",
          },
          workspace: {
            type: "string",
            description: "Workspace path (defaults to current workspace)",
          },
        },
        required: ["snapshot"],
      },
      async execute(_toolCallId, params) {
        const r = await handleDelete(
          params.snapshot as string,
          params.workspace as string | undefined,
        );
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-delete" },
  );

  // --- ws-ckpt-status ---
  api.registerTool(
    {
      name: "ws-ckpt-status",
      description: "Show ws-ckpt service status and workspace information. Returns the complete status from ws-ckpt daemon — no additional CLI or exec verification needed.",
      parameters: { type: "object", properties: {} },
      async execute() {
        const r = await handleStatus();
        return textToolResult(r.text, r.isError);
      },
    },
    { name: "ws-ckpt-status" },
  );
}
