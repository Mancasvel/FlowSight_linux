import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  assertSafePrivacyExportProjections,
  CURRENT_PRIVACY_NOTICE_VERSION,
  PRIVACY_EXPORT_SOURCES,
} from "./privacy_policy.ts";

test("privacy export projections exclude stored credentials and OAuth state", () => {
  assert.doesNotThrow(() => assertSafePrivacyExportProjections());
  const projected = JSON.stringify(PRIVACY_EXPORT_SOURCES);
  for (
    const forbidden of [
      "jira_tokens",
      "state_hash",
      "token_ciphertext",
      "token_iv",
      "encryption_version",
    ]
  ) {
    assert.doesNotMatch(projected, new RegExp(forbidden));
  }
});

test("privacy notice version gates cloud AI and cloud activity processing", async () => {
  const coach = await readFile(
    "supabase/functions/coach-chat/index.ts",
    "utf8",
  );
  const insights = await readFile(
    "supabase/functions/generate-insights/index.ts",
    "utf8",
  );
  assert.match(coach, /CURRENT_PRIVACY_NOTICE_VERSION/);
  assert.match(coach, /cloud_ai_enabled !== true/);
  const coachGate = coach.indexOf("cloud_ai_enabled !== true");
  assert.ok(
    coachGate < coach.indexOf("callAzureCoach(", coachGate),
  );
  assert.match(insights, /CURRENT_PRIVACY_NOTICE_VERSION/);
  assert.match(insights, /cloud_sync_enabled !== true/);
  const insightsGate = insights.indexOf("cloud_sync_enabled !== true");
  assert.ok(
    insightsGate < insights.indexOf("callOpenRouterPmReport(", insightsGate),
  );
  assert.equal(CURRENT_PRIVACY_NOTICE_VERSION, "2026-08-23");
});

test("account deletion blocks ownership cascades before erasing data", async () => {
  const source = await readFile(
    "supabase/functions/privacy-rights/index.ts",
    "utf8",
  );
  const ownershipGate = source.indexOf("ownership_transfer_required");
  const dataDeletion = source.indexOf("deletePseudonymousData(", ownershipGate);
  const authDeletion = source.indexOf("auth.admin.deleteUser", ownershipGate);
  assert.ok(ownershipGate > 0);
  assert.ok(dataDeletion > ownershipGate);
  assert.ok(authDeletion > dataDeletion);
});

test("Notion withdrawal remains ahead of the subscription gate", async () => {
  const source = await readFile(
    "supabase/functions/notion-oauth/index.ts",
    "utf8",
  );
  const disconnect = source.indexOf('action === "disconnect"');
  const status = source.indexOf('action === "status"');
  const entitlement = source.indexOf('"get_user_entitlements"', status);
  assert.ok(disconnect > 0);
  assert.ok(status > disconnect);
  assert.ok(entitlement > status);
});

test("tracked migrations remain non-destructive and deny blanket client privileges", async () => {
  const notionMigration = await readFile(
    "supabase/migrations/20260822120000_notion_pro_reports.sql",
    "utf8",
  );
  const privacyMigration = await readFile(
    "supabase/migrations/20260823120000_gdpr_privacy_controls.sql",
    "utf8",
  );

  for (const source of [notionMigration, privacyMigration]) {
    assert.doesNotMatch(source, /\bdrop\s+table\b/i);
    for (const statement of source.split(";")) {
      if (/^\s*grant\s+all\b/i.test(statement)) {
        assert.doesNotMatch(statement, /\b(?:anon|authenticated)\b/i);
      }
    }
  }

  assert.match(privacyMigration, /as restrictive for insert to authenticated/i);
  assert.match(privacyMigration, /cloud_sync_permitted\(user_id\)/i);
  assert.match(
    privacyMigration,
    /revoke update, delete, truncate, references, trigger on table public\.work_sessions from authenticated/i,
  );
  assert.match(
    privacyMigration,
    /revoke update, delete, truncate, references, trigger on table public\.activity_reports from authenticated/i,
  );
});
