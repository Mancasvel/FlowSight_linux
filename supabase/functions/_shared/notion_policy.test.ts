import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  formatCanonicalNotionReport,
  publicationKey,
  requireNotionPro,
} from "./notion_policy.ts";
import { canonicalFixture } from "./notion_test_fixture.ts";

test("Notion Pro gate rejects non-Pro users explicitly", () => {
  const free = requireNotionPro({ plan: null, status: "free", features: { integrations: false } });
  assert.deepEqual(free, {
    allowed: false,
    status: 403,
    code: "notion_requires_pro",
    error: "Notion publishing requires an active FlowSight Pro plan.",
  });
  assert.equal(requireNotionPro({
    plan: "individual",
    status: "active",
    features: { integrations: true },
  }).allowed, true);
  assert.equal(requireNotionPro({
    plan: "individual",
    status: "past_due",
    features: { integrations: true },
  }).allowed, false);
  assert.equal(requireNotionPro({
    plan: "team",
    status: "active",
    features: { integrations: true },
  }).allowed, false);
});

test("formatter consumes canonical focus_semantics and preserves product terminology", () => {
  const formatted = formatCanonicalNotionReport(canonicalFixture());
  assert.match(formatted.markdown, /Focused ≥ 10 min · Deep ≥ 25 min · Extended ≥ 50 min/);
  assert.match(formatted.markdown, /\| Deep minutes \| 51\.7 min \|/);
  assert.match(formatted.markdown, /\| Deep blocks \| 2 \|/);
  assert.match(formatted.markdown, /\| Longest observed block \| 31\.7 min \|/);
  assert.match(formatted.markdown, /\| Fragmentation \| 18\.5% \|/);
  assert.match(formatted.markdown, /\| Explicit theme coverage \| 80% \|/);
  assert.match(formatted.markdown, /Sustained non-work browsing episodes \| 1/);
  assert.match(formatted.markdown, /Planning: 10 min/);
  assert.match(formatted.markdown, /Meeting: 10 min/);
  assert.match(formatted.markdown, /Administration: 5 min/);
  assert.match(formatted.markdown, /Sales: 5 min/);
  assert.match(formatted.markdown, /Research: 5 min/);
  assert.match(formatted.markdown, /Design: 5 min/);
  assert.match(formatted.markdown, /does not measure subjective flow/);
  assert.doesNotMatch(formatted.markdown, /productivity score/i);
  assert.doesNotMatch(formatted.markdown, /999999/);
});

test("formatter refuses to reconstruct Deep Focus from category totals", () => {
  assert.throws(
    () => formatCanonicalNotionReport({
      period_start: "2026-08-16",
      period_end: "2026-08-22",
      category_breakdown: [{ category: "Coding", total_seconds: 999999 }],
    }),
    /focus_semantics is required/,
  );
});

test("publication key is period-specific but stable for a live page", async () => {
  const base = {
    userId: "user-1",
    destinationId: "dest-1",
    policyVersion: "v2",
  };
  const periodA = await publicationKey({
    ...base,
    reportMode: "period_page",
    periodStart: "2026-08-01",
    periodEnd: "2026-08-07",
  });
  const periodB = await publicationKey({
    ...base,
    reportMode: "period_page",
    periodStart: "2026-08-08",
    periodEnd: "2026-08-14",
  });
  const liveA = await publicationKey({
    ...base,
    reportMode: "live_page",
    periodStart: "2026-08-01",
    periodEnd: "2026-08-07",
  });
  const liveB = await publicationKey({
    ...base,
    reportMode: "live_page",
    periodStart: "2026-08-08",
    periodEnd: "2026-08-14",
  });
  assert.notEqual(periodA, periodB);
  assert.equal(liveA, liveB);
  const samePeriodNewPolicy = await publicationKey({
    ...base,
    policyVersion: "v3",
    reportMode: "period_page",
    periodStart: "2026-08-01",
    periodEnd: "2026-08-07",
  });
  assert.equal(periodA, samePeriodNewPolicy);
});

test("publish endpoint checks Pro before body, token, destination, and Notion", async () => {
  const source = await readFile(
    "supabase/functions/publish-notion-report/index.ts",
    "utf8",
  );
  const gate = source.indexOf("requireNotionPro(entitlements)");
  assert.ok(gate > 0);
  for (const laterStep of ["req.json()", 'from("notion_destinations")', 'from("notion_connections")']) {
    assert.ok(source.indexOf(laterStep) > gate, `${laterStep} must happen after the Pro gate`);
  }
});
