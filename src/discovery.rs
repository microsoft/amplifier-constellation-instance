use serde_json::{Value, json};

/// Discovery for alternate UIs, command-line tools, and management providers.
pub fn openapi() -> Value {
    let mut paths = serde_json::Map::new();
    for (path, methods, scope, description) in [
        (
            "/identity",
            vec!["get"],
            "observe",
            "Portable presentation identity and avatar revision; no image bytes in this metadata response.",
        ),
        (
            "/identity/avatar",
            vec!["get", "put", "delete"],
            "admin",
            "Read the authenticated PNG (observe scope), replace with base64 PNG/JPEG/WebP plus expected_revision, or remove with expected_revision in the query. Connected experiences can retrieve inline portraits through the optional ACP identity extension.",
        ),
        (
            "/native-account",
            vec!["get", "delete"],
            "admin",
            "Optional native engine account status and explicit revocation. Credential values are never returned. Signing in does not automatically change the active provider.",
        ),
        (
            "/native-account/login",
            vec!["get", "post", "delete"],
            "admin",
            "Inspect, start, or cancel a native OAuth job. The callback is loopback only; a browser on another computer needs SSH forwarding. Authorization links are transient; interrupted or expired jobs are not replayed.",
        ),
        (
            "/native-account/models",
            vec!["post"],
            "admin",
            "Retrieve the connected native account's model catalog without changing the configured provider or model.",
        ),
        (
            "/native-config",
            vec!["get", "put"],
            "admin",
            "Optional native engine configuration. GET inspects a revision; PUT validates and atomically updates it through the engine CLI. Existing ACP connections must reconnect; no automatic restart.",
        ),
        (
            "/native-config/schema",
            vec!["get"],
            "admin",
            "Discover the installed native engine version's configuration schema and provider presets.",
        ),
        (
            "/native-config/credential",
            vec!["post"],
            "admin",
            "Connect a private model credential to the native engine.",
        ),
        (
            "/native-config/validate",
            vec!["post"],
            "admin",
            "Validate native engine configuration without modifying it.",
        ),
        (
            "/memory",
            vec!["get", "post"],
            "memory",
            "List or remember audience-scoped attributed knowledge. Different statements under the same topic become explicit conflicts.",
        ),
        (
            "/memory/{id}",
            vec!["get"],
            "memory:read",
            "Read one memory, its alternatives, provenance and revision history.",
        ),
        (
            "/memory/{id}/commands",
            vec!["post"],
            "memory",
            "Resolve or archive a memory using expected_revision.",
        ),
        (
            "/scheduler",
            vec!["get", "put"],
            "work:read (GET); work:execute (PUT)",
            "Inspect or configure durable local ACP execution. Settings include expected_revision,enabled,paused,concurrency,approved_backend_sha256,optional session_mode. Explicit mode must be offered and set successfully before prompt; null keeps engine default. Pausing stops new starts; disabling interrupts active runs. Explicit backend approval is required; transport cannot enforce built-in tool restrictions.",
        ),
        (
            "/work/{id}/execution",
            vec!["get", "post"],
            "work:read (GET); work:execute (POST)",
            "Inspect reports or queue/cancel/reconcile/retry explicit bounded execution. New or explicitly resumed ACP session; 1–12 turns share total time/output/tool limits. Interrupted side effects require reconciliation, never automatic replay. Successful reports remain unverified and move unchanged work to review. Accepting a responsibility alone grants no execution authority.",
        ),
        (
            "/organization",
            vec!["get", "put"],
            "work:read (GET); admin (PUT)",
            "Local template snapshots, assignments and explicit peer sharing policy. PUT {expected_revision,organization}. Policy never grants ACP or peer-machine access.",
        ),
        (
            "/work",
            vec!["get", "post"],
            "work:read (GET); work (POST)",
            "List durable local responsibilities or create proposed work. Work scope also permits reads. Creation records intent; it does not execute a prompt.",
        ),
        (
            "/work/{id}",
            vec!["get"],
            "work:read",
            "Inspect goal, captured assignment/template, audience, state, evidence and revision history.",
        ),
        (
            "/work/{id}/commands",
            vec!["post"],
            "work",
            "Apply {expected_revision,command:{type:...}}. Completion requires reviewed evidence; checks are authenticated reports, not independent validation.",
        ),
        (
            "/work/{id}/export",
            vec!["post"],
            "work",
            "Prepare named-peer envelope with expected_revision,relationship_id,disclosure (summary/details),summary. No message is sent; recipient cannot forward imported material.",
        ),
        (
            "/work/import",
            vec!["post"],
            "work",
            "Receive {relationship_id,assignment_id,packet} through an authorized local operator. Deduplicated by source/id. Remote claims remain unverified proposed work.",
        ),
        (
            "/profile-use",
            vec!["get"],
            "observe",
            "Context delivery for the latest started turn, optionally selected by session_id and max_age_seconds. Unknown/partial/stale are explicit; model behavior is never inferred.",
        ),
        (
            "/auth/login",
            vec!["post"],
            "public",
            "Authenticate the owning OS account using native OS authentication; returns an HTTPS-only session cookie and CSRF token.",
        ),
        (
            "/auth/session",
            vec!["get"],
            "observe",
            "Read the current browser session and CSRF token.",
        ),
        (
            "/auth/logout",
            vec!["post"],
            "observe",
            "Invalidate the current browser session.",
        ),
        (
            "/descriptor",
            vec!["get"],
            "observe",
            "Discover management capabilities and ACP connection details.",
        ),
        (
            "/schema",
            vec!["get"],
            "observe",
            "Get the complete configuration JSON Schema for this binary version.",
        ),
        (
            "/openapi.json",
            vec!["get"],
            "observe",
            "Get this API description.",
        ),
        (
            "/status",
            vec!["get"],
            "observe",
            "Read desired/effective configuration status, uptime, connections, sessions and endpoint details.",
        ),
        (
            "/context/providers",
            vec!["get", "put"],
            "admin",
            "List or replace optional conversation source registry. PUT accepts {providers:[{id,label,url,secret_ref}]}; credentials use private named secrets. Sources are accessed through scoped, agent-owned MCP tools, not ACP extensions.",
        ),
        (
            "/config",
            vec!["get", "put"],
            "admin",
            "Inspect or atomically replace configuration using expected_revision. Instance ID and OS owner are immutable.",
        ),
        (
            "/config/validate",
            vec!["post"],
            "admin",
            "Validate a bare configuration without applying it.",
        ),
        (
            "/lifecycle",
            vec!["post"],
            "admin",
            "Enable or stop agent execution. The management service stays available.",
        ),
        (
            "/profiles/{name}",
            vec!["get", "put"],
            "admin",
            "Inspect or replace communication, working-style, or identity profile text.",
        ),
        (
            "/profile-package",
            vec!["get", "post"],
            "profiles",
            "Read installed style version/history or atomically apply a portable profile. POST {expected_revision,package}; validates file digests, preserves private identity and records rollback history.",
        ),
        (
            "/profile-package/restore",
            vec!["post"],
            "profiles",
            "Restore a previous local style snapshot. POST {expected_revision,id}. Subsequent prompts use the selected pair; in-flight turns and existing transcripts are unchanged.",
        ),
        (
            "/secrets",
            vec!["get"],
            "admin",
            "List configured secret names, never their values.",
        ),
        (
            "/secrets/{name}",
            vec!["put", "delete"],
            "admin",
            "Set or delete a named credential. Values are write-only; control-token is reserved.",
        ),
        (
            "/tokens",
            vec!["get", "post"],
            "admin",
            "List grants or create a scoped, expiring token whose value is returned once.",
        ),
        (
            "/tokens/{id}",
            vec!["delete"],
            "admin",
            "Revoke a grant. Active authenticated streams recheck grants within 15 seconds.",
        ),
        (
            "/connections",
            vec!["get"],
            "observe",
            "List active ACP backend connections and metadata.",
        ),
        (
            "/sessions",
            vec!["get"],
            "observe",
            "List observed backend sessions. Transcript persistence remains the backend's responsibility.",
        ),
        (
            "/sessions/{id}",
            vec!["get"],
            "observe",
            "Read metadata for one observed session.",
        ),
        (
            "/activity",
            vec!["get"],
            "observe",
            "Recent useful activity, newest first. Filter by category/search; diagnostics are opt-in. Use before and next_before for pagination.",
        ),
        (
            "/backend",
            vec!["get"],
            "observe",
            "Default runtime readiness, private account availability and supported setup actions. Availability is not a claim that model access was verified.",
        ),
        (
            "/backend/install",
            vec!["post"],
            "admin",
            "Install this version's private, pinned Codex runtime. Reuses only the owner's account cache unless reuse_owner_account=false. Applies configuration using revision checks.",
        ),
        (
            "/backend/account",
            vec!["get"],
            "admin",
            "Read provider account availability without returning account identifiers or credentials.",
        ),
        (
            "/backend/account/login",
            vec!["get", "post", "delete"],
            "admin",
            "Inspect, start, or cancel device-code account connection. Codes are transient and only returned to administrators.",
        ),
        (
            "/events",
            vec!["get"],
            "observe or ui",
            "Read up to 500 metadata events after a sequence number. UI-only grants see view events only.",
        ),
        (
            "/events/stream",
            vec!["get"],
            "observe or ui",
            "Stream metadata events using SSE. Last-Event-ID and after support resumption within retained history.",
        ),
        (
            "/views",
            vec!["get", "post"],
            "ui",
            "List displays or register a new view. Each browser tab has its own ID.",
        ),
        (
            "/views/{id}",
            vec!["get", "patch"],
            "ui",
            "Inspect or replace a display state with expected_revision.",
        ),
        (
            "/views/{id}/commands",
            vec!["post"],
            "ui",
            "Apply a semantic, revisioned view command. This does not confer configuration or execution authority.",
        ),
        (
            "/views/{id}/ack",
            vec!["post"],
            "ui",
            "A browser reports whether it rendered the requested view revision.",
        ),
        (
            "/acp",
            vec!["get"],
            "acp",
            "WebSocket upgrade using a bearer token. Custom agent-instance.wss/1 framing, not the standardized ACP transport.",
        ),
    ] {
        let mut item = serde_json::Map::new();
        for method in methods {
            let scope = match (path, method) {
                ("/identity/avatar", "get") => "observe",
                ("/memory", "get") => "memory:read",
                ("/scheduler", "get") | ("/work/{id}/execution", "get") => "work:read",
                ("/scheduler", "put") | ("/work/{id}/execution", "post") => "work:execute",
                ("/organization", "get") | ("/work", "get") => "work:read",
                ("/organization", "put") => "admin",
                ("/work", "post") => "work",
                _ => scope,
            };
            let mut operation = json!({"description":description,"x-required-scope":scope,"security":if scope=="public" {json!([])}else{json!([{"bearer":[]},{"browserSession":[]}])},"responses":{"200":{"description":"Success; JSON except the SSE and WebSocket endpoints"},"400":{"description":"Invalid request"},"401":{"description":"Authentication required"},"403":{"description":"Scope, origin or CSRF check failed"},"409":{"description":"Revision conflict or incompatible runtime state"}}});
            if path == "/identity/avatar" && method == "delete" {
                operation["parameters"] = json!([{"name":"expected_revision","in":"query","required":true,"schema":{"type":"integer","minimum":0}}]);
            }
            if path == "/identity/avatar" && method == "get" {
                operation["responses"]["200"] = json!({"description":"Authenticated PNG","content":{"image/png":{"schema":{"type":"string","format":"binary"}}}});
            }
            if path.contains('{') {
                let name = if path.contains("{name}") {
                    "name"
                } else {
                    "id"
                };
                operation["parameters"] =
                    json!([{"name":name,"in":"path","required":true,"schema":{"type":"string"}}]);
            }
            if path.starts_with("/events") {
                operation["parameters"] =
                    json!([{"name":"after","in":"query","schema":{"type":"integer","default":0}}]);
            }
            if path == "/activity" {
                operation["parameters"] = json!([
                    {"name":"limit","in":"query","schema":{"type":"integer","default":100,"minimum":1,"maximum":200}},
                    {"name":"before","in":"query","schema":{"type":"integer"}},
                    {"name":"diagnostics","in":"query","schema":{"type":"boolean","default":false}},
                    {"name":"category","in":"query","schema":{"type":"string","default":"all"}},
                    {"name":"search","in":"query","schema":{"type":"string"}}
                ]);
            }
            if ["post", "put", "patch"].contains(&method)
                || (method == "delete"
                    && matches!(path, "/native-account" | "/native-account/login"))
            {
                let mut schema = match path {
                    "/identity/avatar" => json!(schemars::schema_for!(crate::identity::Upload)),
                    "/native-account" => {
                        json!(schemars::schema_for!(crate::native_account::Disconnect))
                    }
                    "/native-account/login" if method == "delete" => {
                        json!(schemars::schema_for!(crate::native_account::Cancel))
                    }
                    "/native-account/login" => {
                        json!(schemars::schema_for!(crate::native_account::Start))
                    }
                    "/organization" => {
                        json!(schemars::schema_for!(crate::work::OrganizationUpdate))
                    }
                    "/native-config/credential" => {
                        json!(schemars::schema_for!(crate::native_config::Credential))
                    }
                    "/native-config" | "/native-config/validate" => {
                        json!(schemars::schema_for!(crate::native_config::Update))
                    }
                    "/memory" => json!(schemars::schema_for!(crate::memory::Remember)),
                    "/memory/{id}/commands" => json!(schemars::schema_for!(crate::memory::Command)),
                    "/scheduler" => json!(schemars::schema_for!(crate::scheduler::Settings)),
                    "/work/{id}/execution" => {
                        json!(schemars::schema_for!(crate::scheduler::Command))
                    }
                    "/work" => json!(schemars::schema_for!(crate::work::NewWork)),
                    "/work/{id}/commands" => json!(schemars::schema_for!(crate::work::Command)),
                    "/work/{id}/export" => json!(schemars::schema_for!(crate::work::Export)),
                    "/work/import" => json!(schemars::schema_for!(crate::work::Import)),
                    "/backend/install" => {
                        json!({"type":"object","properties":{"engine":{"type":"string","enum":["native","codex"],"default":"native"},"reuse_owner_account":{"type":"boolean","default":true}}})
                    }
                    "/auth/login" => {
                        json!({"type":"object","required":["username","password"],"properties":{"username":{"type":"string"},"password":{"type":"string","writeOnly":true}}})
                    }
                    "/config" => {
                        json!({"type":"object","required":["expected_revision","config"],"properties":{"expected_revision":{"type":"integer"},"config":{"$ref":"./schema"}}})
                    }
                    "/config/validate" => json!({"$ref":"./schema"}),
                    "/lifecycle" => {
                        json!({"type":"object","required":["action"],"properties":{"action":{"enum":["start","stop"]}}})
                    }
                    "/profile-package" => {
                        json!({"type":"object","required":["expected_revision","package"],"properties":{"expected_revision":{"type":"integer","minimum":0},"package":{"type":"object","required":["schema","manifest","files"]}}})
                    }
                    "/profile-package/restore" => {
                        json!({"type":"object","required":["expected_revision","id"],"properties":{"expected_revision":{"type":"integer","minimum":0},"id":{"type":"string","format":"uuid"}}})
                    }
                    "/profiles/{name}" => {
                        json!({"type":"object","required":["text"],"properties":{"text":{"type":"string","maxLength":131072}}})
                    }
                    "/secrets/{name}" => {
                        json!({"type":"object","required":["value"],"properties":{"value":{"type":"string","writeOnly":true,"maxLength":65536}}})
                    }
                    "/tokens" => {
                        json!({"type":"object","required":["name","scopes","expires_in_seconds"],"properties":{"name":{"type":"string"},"scopes":{"type":"array","items":{"enum":["admin","acp","observe","ui","profiles","context:owner","work:read","work","work:execute","memory:read","memory"]}},"expires_in_seconds":{"type":"integer","minimum":60,"maximum":31622400}}})
                    }
                    "/views" => json!({"type":"object","properties":{"label":{"type":"string"}}}),
                    "/views/{id}" => {
                        json!({"type":"object","required":["expected_revision","state"],"properties":{"expected_revision":{"type":"integer"},"state":{"type":"object"}}})
                    }
                    "/views/{id}/commands" => {
                        json!({"type":"object","required":["expected_revision","command"],"properties":{"expected_revision":{"type":"integer"},"command":{"type":"object","required":["type"],"properties":{"type":{"enum":["navigate","set_draft","focus","scroll","select_session","set_filter","open_modal","close_modal"]}}}}})
                    }
                    "/views/{id}/ack" => {
                        json!({"type":"object","required":["revision","status"],"properties":{"revision":{"type":"integer"},"status":{"enum":["applied","error"]},"detail":{"type":"string"}}})
                    }
                    _ => json!({"type":"object"}),
                };
                // Each generated request schema is its own JSON Schema resource,
                // so its local #/$defs references resolve within this body schema.
                if schema.get("$defs").is_some() {
                    schema["$id"] = json!(format!(
                        "urn:agent-instance:request:{}:1",
                        path.replace('/', ":").replace(['{', '}'], "")
                    ));
                }
                operation["requestBody"] =
                    json!({"required":true,"content":{"application/json":{"schema":schema}}});
            }
            item.insert(method.into(), operation);
        }
        paths.insert(path.into(), Value::Object(item));
    }
    json!({"openapi":"3.1.0","info":{"title":"Agent Instance management","version":env!("CARGO_PKG_VERSION"),"description":"Versioned management API separate from ACP. Admin grants include every scope. Cookie-authenticated mutations require X-CSRF-Token. Base ACP clients use the stdio attach command."},"servers":[{"url":"./"}],"paths":paths,"components":{"securitySchemes":{"bearer":{"type":"http","scheme":"bearer"},"browserSession":{"type":"apiKey","in":"cookie","name":"ai_<instance-id-without-dashes>","description":"Instance-specific, Secure, HttpOnly, SameSite=Strict cookie returned by native OS account sign-in"}}}})
}
