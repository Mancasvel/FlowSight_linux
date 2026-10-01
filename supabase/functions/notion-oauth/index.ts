import {
  authenticate,
  corsHeaders,
  errorResponse,
  isResponse,
  jsonResponse,
} from "../_shared/http.ts";
import {
  encryptTokenBundle,
  exchangeAuthorizationCode,
} from "../_shared/notion_api.ts";
import { requireNotionPro, sha256Hex } from "../_shared/notion_policy.ts";

const STATE_TTL_MS = 10 * 60 * 1000;

function htmlResponse(title: string, message: string, ok: boolean): Response {
  const color = ok ? "#16794b" : "#b42318";
  const html =
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><title>${title}</title></head><body style="font-family:system-ui,sans-serif;max-width:560px;margin:64px auto;padding:24px;color:#17211b"><h1 style="color:${color}">${title}</h1><p>${message}</p><p>You can close this window and return to FlowSight.</p></body></html>`;
  return new Response(html, {
    status: ok ? 200 : 400,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
    },
  });
}

function base64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll(
    "=",
    "",
  );
}

async function handleCallback(url: URL): Promise<Response> {
  if (url.searchParams.has("error")) {
    return htmlResponse(
      "Notion was not connected",
      "Authorization was cancelled or denied.",
      false,
    );
  }
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  if (!code || !state) {
    return htmlResponse(
      "Invalid Notion callback",
      "The authorization response was incomplete.",
      false,
    );
  }

  const separator = state.indexOf(".");
  const stateId = separator > 0 ? state.slice(0, separator) : "";
  const secret = separator > 0 ? state.slice(separator + 1) : "";
  if (!/^[0-9a-f-]{36}$/i.test(stateId) || secret.length < 32) {
    return htmlResponse(
      "Invalid Notion callback",
      "The security state was invalid.",
      false,
    );
  }

  const supabaseUrl = Deno.env.get("SUPABASE_URL");
  const serviceKey = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY");
  const clientId = Deno.env.get("NOTION_CLIENT_ID");
  const clientSecret = Deno.env.get("NOTION_CLIENT_SECRET");
  const redirectUri = Deno.env.get("NOTION_REDIRECT_URI");
  const encryptionKey = Deno.env.get("NOTION_TOKEN_ENCRYPTION_KEY");
  if (
    !supabaseUrl || !serviceKey || !clientId || !clientSecret || !redirectUri ||
    !encryptionKey
  ) {
    return htmlResponse(
      "Notion is unavailable",
      "The FlowSight integration is not configured.",
      false,
    );
  }

  const { createClient } = await import(
    "https://esm.sh/@supabase/supabase-js@2.49.1"
  );
  const serviceClient = createClient(supabaseUrl, serviceKey, {
    auth: { persistSession: false },
  });
  const stateHash = await sha256Hex(secret);
  const { data: consumed, error: consumeError } = await serviceClient.rpc(
    "consume_notion_oauth_state",
    { p_state_id: stateId, p_state_hash: stateHash },
  );
  const userId = Array.isArray(consumed) ? consumed[0]?.user_id : null;
  if (consumeError || typeof userId !== "string") {
    return htmlResponse(
      "Notion link expired",
      "Start the connection again from FlowSight.",
      false,
    );
  }

  try {
    const token = await exchangeAuthorizationCode({
      code,
      clientId,
      clientSecret,
      redirectUri,
    });
    const encrypted = await encryptTokenBundle({
      access_token: token.access_token as string,
      ...(typeof token.refresh_token === "string"
        ? { refresh_token: token.refresh_token }
        : {}),
      ...(typeof token.token_type === "string"
        ? { token_type: token.token_type }
        : {}),
    }, encryptionKey);
    const workspaceId = typeof token.workspace_id === "string"
      ? token.workspace_id
      : null;
    if (!workspaceId) throw new Error("Notion did not return a workspace id.");

    const { error } = await serviceClient.from("notion_connections").upsert({
      user_id: userId,
      workspace_id: workspaceId,
      workspace_name: typeof token.workspace_name === "string"
        ? token.workspace_name
        : null,
      workspace_icon: typeof token.workspace_icon === "string"
        ? token.workspace_icon
        : null,
      bot_id: typeof token.bot_id === "string" ? token.bot_id : null,
      token_ciphertext: encrypted.ciphertext,
      token_iv: encrypted.iv,
      encryption_version: encrypted.version,
      connected_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    }, { onConflict: "user_id" });
    if (error) throw new Error("Could not persist the Notion connection.");
    return htmlResponse(
      "Notion connected",
      "FlowSight can now publish to the pages you shared.",
      true,
    );
  } catch {
    return htmlResponse(
      "Notion was not connected",
      "The secure token exchange failed. Try again from FlowSight.",
      false,
    );
  }
}

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") {
    return new Response("ok", { headers: corsHeaders });
  }
  const url = new URL(req.url);
  if (req.method === "GET") return handleCallback(url);
  if (req.method !== "POST") {
    return errorResponse("Method not allowed.", 405, "method_not_allowed");
  }

  const auth = await authenticate(req);
  if (isResponse(auth)) return auth;
  const body = await req.json().catch(() => ({})) as Record<string, unknown>;
  const action = body.action;
  if (action === "disconnect") {
    const connectionDelete = await auth.serviceClient.from("notion_connections")
      .delete()
      .eq("user_id", auth.userId);
    if (connectionDelete.error) {
      return errorResponse(
        "Could not disconnect Notion.",
        500,
        "disconnect_failed",
      );
    }
    const stateDelete = await auth.serviceClient.from("notion_oauth_states")
      .delete()
      .eq("user_id", auth.userId);
    if (stateDelete.error) {
      return errorResponse(
        "The connection was removed but pending authorization state could not be cleared.",
        500,
        "state_cleanup_failed",
      );
    }
    return jsonResponse({ disconnected: true });
  }

  // Status and disconnection remain available even if a subscription expires,
  // so a data subject never has to pay to review or withdraw an integration.
  if (action === "status") {
    const [
      { data: connection },
      { data: destinations },
      { data: publication },
    ] = await Promise.all([
      auth.serviceClient.from("notion_connections")
        .select(
          "workspace_id,workspace_name,workspace_icon,connected_at,updated_at",
        )
        .eq("user_id", auth.userId).maybeSingle(),
      auth.serviceClient.from("notion_destinations")
        .select(
          "id,notion_object_id,destination_type,title,report_mode,is_default,updated_at",
        )
        .eq("user_id", auth.userId).order("updated_at", { ascending: false }),
      auth.serviceClient.from("notion_publications")
        .select(
          "id,status,period_start,period_end,notion_page_url,published_at,updated_at",
        )
        .eq("user_id", auth.userId).eq("status", "published")
        .order("updated_at", { ascending: false }).limit(1).maybeSingle(),
    ]);
    return jsonResponse({
      connected: Boolean(connection),
      connection: connection ?? null,
      destinations: destinations ?? [],
      last_publication: publication ?? null,
    });
  }

  const { data: entitlements, error: entitlementError } = await auth.userClient
    .rpc(
      "get_user_entitlements",
    );
  if (entitlementError) {
    return errorResponse(
      "Could not verify your plan.",
      503,
      "entitlement_check_failed",
    );
  }
  const gate = requireNotionPro(entitlements);
  if (!gate.allowed) return errorResponse(gate.error, gate.status, gate.code);

  if (action === "start") {
    const clientId = Deno.env.get("NOTION_CLIENT_ID");
    const redirectUri = Deno.env.get("NOTION_REDIRECT_URI");
    if (!clientId || !redirectUri) {
      return errorResponse(
        "Notion OAuth is not configured.",
        503,
        "notion_not_configured",
      );
    }
    const stateId = crypto.randomUUID();
    const secret = base64Url(crypto.getRandomValues(new Uint8Array(32)));
    const stateHash = await sha256Hex(secret);
    const { error } = await auth.serviceClient.from("notion_oauth_states")
      .insert({
        id: stateId,
        user_id: auth.userId,
        state_hash: stateHash,
        expires_at: new Date(Date.now() + STATE_TTL_MS).toISOString(),
      });
    if (error) {
      return errorResponse(
        "Could not start Notion authorization.",
        500,
        "oauth_state_failed",
      );
    }

    const authorizationUrl = new URL(
      "https://api.notion.com/v1/oauth/authorize",
    );
    authorizationUrl.searchParams.set("owner", "user");
    authorizationUrl.searchParams.set("client_id", clientId);
    authorizationUrl.searchParams.set("redirect_uri", redirectUri);
    authorizationUrl.searchParams.set("response_type", "code");
    authorizationUrl.searchParams.set("state", `${stateId}.${secret}`);
    return jsonResponse({
      authorization_url: authorizationUrl.toString(),
      expires_in_seconds: 600,
    });
  }

  return errorResponse("Unknown action.", 400, "invalid_action");
});
