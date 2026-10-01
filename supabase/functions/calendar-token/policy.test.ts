import assert from "node:assert/strict";
import test from "node:test";
import {
  hasPaidCalendarAccess,
  parseCalendarTokenRequest,
  validLoopbackRedirect,
} from "./policy.ts";

test("Calendar token broker requires an eligible active Cloud integrations plan", () => {
  assert.equal(hasPaidCalendarAccess({
    plan: "individual_pro", status: "active", features: { integrations: true },
  }), true);
  assert.equal(hasPaidCalendarAccess({
    plan: "individual", status: "past_due", features: { integrations: true },
  }), false);
  assert.equal(hasPaidCalendarAccess({
    plan: "team", status: "active", features: { integrations: true },
  }), false);
  assert.equal(hasPaidCalendarAccess({
    plan: "individual_local", status: "active", features: { integrations: true },
  }), false);
  assert.equal(hasPaidCalendarAccess(null), false);
});

test("Calendar token broker accepts only local callback URLs", () => {
  assert.equal(validLoopbackRedirect("http://127.0.0.1:54555/callback"), true);
  assert.equal(validLoopbackRedirect("http://localhost:54555/callback"), true);
  for (const url of [
    "https://127.0.0.1:54555/callback",
    "http://evil.example:54555/callback",
    "http://127.0.0.1/callback",
    "http://127.0.0.1:80/callback",
    "http://127.0.0.1:54555/other",
    "http://user@127.0.0.1:54555/callback",
    "http://127.0.0.1:54555/callback?next=evil",
  ]) assert.equal(validLoopbackRedirect(url), false, url);
});

test("Calendar token broker validates exchange and refresh payloads", () => {
  const exchange = {
    action: "exchange",
    code: "4/0AX4XfWj-opaque-code",
    code_verifier: "a".repeat(43),
    redirect_uri: "http://127.0.0.1:54555/callback",
  };
  assert.deepEqual(parseCalendarTokenRequest(exchange), exchange);
  assert.equal(parseCalendarTokenRequest({ ...exchange, code_verifier: "short" }), null);
  assert.equal(parseCalendarTokenRequest({ ...exchange, redirect_uri: "https://evil.example" }), null);
  assert.deepEqual(parseCalendarTokenRequest({
    action: "refresh", refresh_token: "1//0g-opaque-refresh-token",
  }), { action: "refresh", refresh_token: "1//0g-opaque-refresh-token" });
  assert.equal(parseCalendarTokenRequest({ action: "refresh", refresh_token: "bad token" }), null);
});
