# Official Codex app-server adapter prototype

This optional external ACP engine connects a local, single-owner Instance to an installed official Codex app server. Codex owns its managed ChatGPT subscription login and inference. The adapter stores chat bindings, operation receipts, and original image bytes. Instance continues to select and launch the configured engine. The prototype has no installer or default route.

Use Python 3.10 or later on a Unix host with the full Codex package. The qualification used Codex 0.171.0 and its experimental account-session protocol. Give the adapter a dedicated Codex profile with file-backed sign-in, an owner-only state directory, and absolute paths in the Instance external backend configuration.

```sh
python3 adapter.py --binary /path/to/codex --profile /path/to/codex-home \
  --directory /path/to/private-state
```

Use `--images` to enable Codex image generation and inspection. Use `--workspace-write` only when the owner permits the Codex process to write in its sandbox workspace. The default sandbox is read-only. Host text file callbacks additionally require the corresponding ACP client capability. The client authorizes and resolves its own paths, including symlinks. Host MCP servers and general ACP content parity remain outside the prototype.

The client supplies one UUID in `session/prompt.params._meta.operationId` per logical instruction and reuses it on retries. Recovery reads the bound Codex thread and returns stored originals by SHA-256. An uncertain turn blocks automatic replay. The owner's `artifact.py` command can export an original, create a smaller preview using Pillow, or accept a recovered image on an idle thread. All local journals contain private chat or image data and need the same protection as the profile.

An external workspace change can leave a running Codex process reporting its old selection. The adapter checks for replacement or modification of the managed login file during I/O and callback waits, stops that process and requires a fresh identity check. Token refresh can also require reconnecting. This is a local generation check, with no atomic guarantee against an external change at the instant of dispatch. Cancellation waits for Codex to confirm interruption. Neither stopping the process nor cancelling a turn rolls back an effect already issued to a host.

## Reproduce the checks

Run `python3 -m unittest discover -v` here for the identity checks. The live scripts use subscription quota and disposable profiles, workspaces and sessions. Pass the real Instance ACP client command after `--`.

- `host_test.py` generates and edits a print and reconnects after the client drops the final reply.
- `interaction_test.py host` exercises granted host reads and writes, interruption with a callback outstanding, a late reply and a repeated operation ID. Use a write-enabled disposable route for this case.
- `interaction_test.py switch` changes the selected workspace in a second official Codex process during a pending callback. It requires two accessible workspaces and restores the first selection. Both interaction tests take `--binary`, `--profile`, `--directory`, and `--cwd` pointing to the test backend's configuration.
- `recovery_test.py before-receipt` kills the disposable server after Codex completes an image but before the adapter records it. The `interrupt` variant kills it after the receipt. Run `recover` with the same session, operation and prompt. It refuses another inference request and reloads the recovered bytes. See each script's `--help` for arguments.
