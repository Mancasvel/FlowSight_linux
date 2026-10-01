// Shared by the deployed broker and its local tests.
export type CalendarEntitlements = {
  plan?: string | null;
  status?: string | null;
  features?: { integrations?: boolean } | null;
};

export type CalendarTokenRequest =
  | { action: "exchange"; code: string; code_verifier: string; redirect_uri: string }
  | { action: "refresh"; refresh_token: string };

// The one-time Individual local purchase is stored separately and grants no Cloud entitlement.
const PAID_PLANS = new Set(["pro", "individual", "individual_pro"]);

export function hasPaidCalendarAccess(entitlements: CalendarEntitlements | null | undefined): boolean {
  return entitlements?.status === "active" &&
    PAID_PLANS.has(entitlements.plan ?? "") &&
    entitlements.features?.integrations === true;
}

function opaqueToken(value: unknown): value is string {
  return typeof value === "string" &&
    value.length >= 10 && value.length <= 4096 &&
    /^[\x21-\x7e]+$/.test(value);
}

export function validLoopbackRedirect(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try {
    const url = new URL(value);
    const port = Number(url.port);
    return url.protocol === "http:" &&
      (url.hostname === "127.0.0.1" || url.hostname === "localhost") &&
      Number.isInteger(port) && port >= 1024 && port <= 65535 &&
      url.pathname === "/callback" &&
      !url.username && !url.password && !url.search && !url.hash;
  } catch {
    return false;
  }
}

export function parseCalendarTokenRequest(value: unknown): CalendarTokenRequest | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const body = value as Record<string, unknown>;
  if (body.action === "exchange") {
    if (!opaqueToken(body.code) ||
      typeof body.code_verifier !== "string" ||
      !/^[A-Za-z0-9._~-]{43,128}$/.test(body.code_verifier) ||
      !validLoopbackRedirect(body.redirect_uri)) return null;
    return {
      action: "exchange",
      code: body.code,
      code_verifier: body.code_verifier,
      redirect_uri: body.redirect_uri,
    };
  }
  if (body.action === "refresh" && opaqueToken(body.refresh_token)) {
    return { action: "refresh", refresh_token: body.refresh_token };
  }
  return null;
}
