import assert from "node:assert/strict";
import test from "node:test";
import { formatCanonicalNotionReport } from "./notion_policy.ts";
import { canonicalFixture } from "./notion_test_fixture.ts";
import {
  coordinatePublication,
  type PublicationRecord,
  type PublicationRepository,
} from "./notion_publish.ts";

test("publishing the same period twice creates exactly one Notion page", async () => {
  let record: PublicationRecord | null = null;
  let createCalls = 0;
  const repository: PublicationRepository = {
    async claim() {
      if (record) return { created: false, record };
      record = { id: "pub-1", status: "pending", notion_page_id: null, notion_page_url: null };
      return { created: true, record };
    },
    async markPublished(_id, pageId, pageUrl) {
      record = { id: "pub-1", status: "published", notion_page_id: pageId, notion_page_url: pageUrl };
    },
    async markFailed() {
      throw new Error("unexpected failure");
    },
  };
  const publisher = {
    async createPage() {
      createCalls += 1;
      return { id: "notion-page-1", url: "https://notion.so/page-1" };
    },
    async replacePage() {
      throw new Error("period pages are not replaced");
    },
  };
  const input = {
    reportMode: "period_page" as const,
    requestId: "same-period-key",
    report: formatCanonicalNotionReport(canonicalFixture()),
    repository,
    publisher,
  };

  const first = await coordinatePublication(input);
  const second = await coordinatePublication(input);
  assert.equal(first.status, "published");
  assert.equal(second.status, "already_published");
  assert.equal(first.notionPageId, second.notionPageId);
  assert.equal(createCalls, 1);
});

test("live mode updates the existing page instead of creating another", async () => {
  let replaceCalls = 0;
  const record: PublicationRecord = {
    id: "pub-live",
    status: "published",
    notion_page_id: "page-live",
    notion_page_url: "https://notion.so/live",
  };
  const result = await coordinatePublication({
    reportMode: "live_page",
    requestId: "stable-live-key",
    report: formatCanonicalNotionReport(canonicalFixture()),
    repository: {
      async claim() {
        return { created: false, record };
      },
      async markPublished() {},
      async markFailed() {
        throw new Error("unexpected failure");
      },
    },
    publisher: {
      async createPage() {
        throw new Error("must not create a second live page");
      },
      async replacePage(pageId) {
        assert.equal(pageId, "page-live");
        replaceCalls += 1;
      },
    },
  });
  assert.equal(result.status, "updated");
  assert.equal(replaceCalls, 1);
});
