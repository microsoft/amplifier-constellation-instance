// Drives our ordinary stdio attach command using the independent official SDK.
// Usage: node official-client.mjs ABSOLUTE_WORKDIR COMMAND [ARGS...]
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { Readable, Writable } from "node:stream";
import * as acp from "@agentclientprotocol/sdk";

const [cwd, command, ...args] = process.argv.slice(2);
if (!cwd || !command) throw new Error("Provide ABSOLUTE_WORKDIR COMMAND [ARGS...]");
const processHandle = spawn(command, args, { stdio: ["pipe", "pipe", "inherit"] });
const timer = setTimeout(() => processHandle.kill("SIGKILL"), 30000);
const stream = acp.ndJsonStream(Writable.toWeb(processHandle.stdin), Readable.toWeb(processHandle.stdout));
const updates = [];
let permissionRequests = 0;
let fileRequests = 0;
try {
  const report = await acp.client({ name: "agent-instance-interoperability-client" })
    .onRequest(acp.methods.client.session.requestPermission, ({ params }) => {
      permissionRequests++;
      assert.equal(params.options[0].optionId, "permit");
      return { outcome: { outcome: "selected", optionId: "permit" } };
    })
    .onRequest(acp.methods.client.fs.readTextFile, () => {
      fileRequests++;
      return { content: "SDK callback fixture" };
    })
    .onNotification(acp.methods.client.session.update, ({ params }) => updates.push(params))
    .connectWith(stream, async (ctx) => {
      const init = await ctx.request(acp.methods.agent.initialize, {
        protocolVersion: acp.PROTOCOL_VERSION,
        clientInfo: { name: "official-sdk-test", version: "2.2.0" },
        clientCapabilities: { fs: { readTextFile: true } },
      });
      assert.equal(init.protocolVersion, 1);
      assert.equal(init.agentInfo.name, "agent-instance");
      assert.equal(init.agentCapabilities.loadSession, true);
      const session = await ctx.request(acp.methods.agent.session.new, { cwd, mcpServers: [] });
      const result = await ctx.request(acp.methods.agent.session.prompt, {
        sessionId: session.sessionId,
        prompt: [
          { type: "text", text: "REQUEST_PERMISSION REQUEST_FILE" },
          { type: "resource_link", uri: "file:///fixture/context", name: "Context" },
        ],
      });
      assert.equal(result.stopReason, "end_turn");
      const detail = JSON.parse(updates.at(-1).update.content.text);
      assert.deepEqual(detail.types, ["text", "text", "resource_link"]);
      assert.equal(detail.permission.outcome.optionId, "permit");
      assert.equal(detail.file_result.content, "SDK callback fixture");
      const listing = await ctx.request(acp.methods.agent.session.list, {});
      assert.ok(listing.sessions.some((item) => item.sessionId === session.sessionId));
      assert.equal(permissionRequests, 1);
      assert.equal(fileRequests, 1);
      return { sdk: "@agentclientprotocol/sdk@2.2.0", protocol: init.protocolVersion,
        session_id: session.sessionId, updates: updates.length, permission_callbacks: permissionRequests,
        filesystem_callbacks: fileRequests, resource_links: true, session_list: true };
    });
  console.log(JSON.stringify({ success: true, ...report }));
} finally {
  clearTimeout(timer);
  processHandle.kill();
}
