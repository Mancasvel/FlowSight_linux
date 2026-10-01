import { createClient, type SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";
import {
  hasPaidCalendarAccess,
  parseCalendarTokenRequest,
} from "./policy.ts";

const corsHeaders = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
};

type AuthContext = { userClient: SupabaseClient };

async function authenticate(req: Request): Promise<AuthContext | Response> {
  const bearer = req.headers.get("Authorization");
  if (!bearer?.startsWith("Bearer ")) {
    return reply({ error: "Authentication is required.", code: "authentication_required" }, 401);
  }
  const supabaseUrl = Deno.env.get("SUPABASE_URL");
  const anonKey = Deno.env.get("SUPABASE_ANON_KEY");
  if (!supabaseUrl || !anonKey) {
    return reply({ error: "Calendar service is not configured.", code: "service_not_configured" }, 503);
  }
  const userClient = createClient(supabaseUrl, anonKey, {
    global: { headers: { Authorization: bearer } },
    auth: { persistSession: false },
  });
  const { data, error } = await userClient.auth.getUser();
  if (error || !data.user) {
    return reply({ error: "Your session is invalid or expired.", code: "invalid_session" }, 401);
  }
  return { userClient };
}

function isResponse(value: AuthContext | Response): value is Response {
  return value instanceof Response;
}

function reply(body: Record<string, unknown>, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      ...corsHeaders,
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      Pragma: "no-cache",
    },
  });
}

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") return new Response("ok", { headers: corsHeaders });
  if (req.method !== "POST") return reply({ error: "Method not allowed.", code: "method_not_allowed" }, 405);

  // Never parse or log OAuth codes, verifiers or refresh tokens before the
  // Supabase identity and the server-side paid entitlement have been checked.
  const auth = await authenticate(req);
  if (isResponse(auth)) return auth;
  const { data: entitlements, error: entitlementError } = await auth.userClient.rpc(
    "get_user_entitlements",
  );
  if (entitlementError) {
    return reply({ error: "Could not verify your FlowSight plan.", code: "entitlement_check_failed" }, 503);
  }
  if (!hasPaidCalendarAccess(entitlements)) {
    return reply({ error: "Calendar requires an eligible active FlowSight Cloud plan with integrations.", code: "calendar_requires_pro" }, 403);
  }

  const clientId = Deno.env.get("GOOGLE_CALENDAR_CLIENT_ID");
  const clientSecret = Deno.env.get("GOOGLE_CALENDAR_CLIENT_SECRET");
  if (!clientId || !clientSecret) {
    return reply({ error: "Google Calendar is not configured.", code: "calendar_not_configured" }, 503);
  }

  const payload = parseCalendarTokenRequest(await req.json().catch(() => null));
  if (!payload) return reply({ error: "Invalid calendar authorization request.", code: "invalid_request" }, 400);

  const form = new URLSearchParams({
    client_id: clientId,
    client_secret: clientSecret,
    grant_type: payload.action === "exchange" ? "authorization_code" : "refresh_token",
  });
  if (payload.action === "exchange") {
    form.set("code", payload.code);
    form.set("code_verifier", payload.code_verifier);
    form.set("redirect_uri", payload.redirect_uri);
  } else {
    form.set("refresh_token", payload.refresh_token);
  }

  let upstream: Response;
  try {
    upstream = await fetch("https://oauth2.googleapis.com/token", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: form,
      signal: AbortSignal.timeout(20_000),
    });
  } catch {
    return reply({ error: "Google could not be reached.", code: "google_unavailable" }, 502);
  }
  const google = await upstream.json().catch(() => null) as Record<string, unknown> | null;
  if (!upstream.ok) {
    // Never echo Google's response body: it may contain OAuth request detail.
    return reply({ error: "Google rejected calendar authorization. Reconnect in Settings.", code: "google_token_rejected" }, 400);
  }
  if (!google || typeof google.access_token !== "string" ||
    typeof google.expires_in !== "number" ||
    (payload.action === "exchange" && typeof google.refresh_token !== "string")) {
    return reply({ error: "Google returned an incomplete token response.", code: "incomplete_token_response" }, 502);
  }
  return reply({
    access_token: google.access_token,
    ...(typeof google.refresh_token === "string" ? { refresh_token: google.refresh_token } : {}),
    expires_in: google.expires_in,
  });
});
