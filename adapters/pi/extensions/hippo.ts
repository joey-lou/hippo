// Pi session adapter. Fails open: a missing hippo binary must not break the session.
// Install: `hippo adapter install pi`, or adapters/pi/install.sh from a checkout.
// The model provider does not matter. OpenRouter and any other Pi backend use
// these same events. `hippo add` and `hippo update` commit and push on their own.
// This adapter injects the digest, syncs memory files edited with Pi's edit and
// write tools, and checks for unsynced work once before Pi settles.

import { homedir } from "node:os";
import { resolve } from "node:path";

const FILE_TOOLS = new Set(["edit", "write"]);

export default function (pi) {
  const hippo = (args, timeout = 20000) => pi.exec("hippo", args, { timeout });
  let nudged = false;

  pi.on("session_start", async () => {
    try {
      await hippo(["reindex", "--changed"]);
      const digest = await hippo(["digest"], 10000);
      const text = (digest.stdout || "").trim();
      if (!text) return;
      pi.sendMessage(
        {
          customType: "hippo-digest",
          content: [
            "Hippo memory digest. This is a count and topic list, not the memories.",
            "Recall with `hippo query \"<text>\" --json` and read only the top files.",
            "Save durable facts with `hippo add` or `hippo update` after a query.",
            "",
            text,
          ].join("\n"),
          display: false,
        },
        { triggerTurn: false, deliverAs: "nextTurn" },
      );
    } catch {
      // Fail open.
    }
  });

  pi.on("before_agent_start", () => {
    nudged = false;
  });

  pi.on("tool_result", async (event, ctx) => {
    if (event.isError || !FILE_TOOLS.has(event.toolName)) return;
    const file = resolve(ctx.cwd, String(event.input.path).replace(/^~(?=\/|$)/, homedir()));
    await hippo(["sync", "--file", file], 30000).catch(() => {});
  });

  pi.on("agent_before_settle", async (event) => {
    if (nudged || event.continue) return;
    const notice = await unsyncedNotice(hippo);
    if (!notice) return;
    nudged = true;
    return {
      entries: [{ type: "custom_message", customType: "hippo-status", content: notice, display: true }],
      continue: true,
    };
  });
}

async function unsyncedNotice(hippo) {
  try {
    const status = JSON.parse((await hippo(["status", "--json"], 15000)).stdout);
    const drift = status.new.length + status.changed.length + status.removed.length;
    const { dirty, ahead } = status.git;
    if (!drift && !dirty && !ahead) return undefined;
    return (
      "Hippo: the memory repo still has unsynced work " +
      `(drift=${drift}, uncommitted=${dirty ? "True" : "False"}, unpushed=${ahead}). ` +
      "Hippo should have committed and pushed this. " +
      "Tell me briefly what is left. Do not commit or push unless I ask."
    );
  } catch {
    return undefined;
  }
}
