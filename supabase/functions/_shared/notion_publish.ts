import type { CanonicalReport } from "./notion_policy.ts";

export type PublicationRecord = {
  id: string;
  status: "pending" | "published" | "failed";
  notion_page_id: string | null;
  notion_page_url: string | null;
};

export interface PublicationRepository {
  claim(): Promise<{ created: boolean; record: PublicationRecord }>;
  markPublished(id: string, pageId: string, pageUrl: string | null): Promise<void>;
  markFailed(id: string, failureCode: string): Promise<void>;
}

export interface ReportPublisher {
  createPage(report: CanonicalReport, requestId: string): Promise<{ id: string; url: string | null }>;
  replacePage(pageId: string, report: CanonicalReport, requestId: string): Promise<void>;
}

export type PublishResult = {
  publicationId: string;
  notionPageId: string | null;
  notionPageUrl: string | null;
  status: "published" | "already_published" | "updated" | "in_progress";
};

function failureCode(error: unknown): string {
  if (error && typeof error === "object" && "code" in error &&
    typeof (error as { code?: unknown }).code === "string") {
    return (error as { code: string }).code.slice(0, 80);
  }
  return "notion_publish_failed";
}

export async function coordinatePublication(input: {
  reportMode: "period_page" | "live_page";
  requestId: string;
  report: CanonicalReport;
  repository: PublicationRepository;
  publisher: ReportPublisher;
}): Promise<PublishResult> {
  const claim = await input.repository.claim();
  const existing = claim.record;

  if (!claim.created && input.reportMode === "period_page" && existing.status === "published") {
    return {
      publicationId: existing.id,
      notionPageId: existing.notion_page_id,
      notionPageUrl: existing.notion_page_url,
      status: "already_published",
    };
  }
  if (!claim.created && existing.status === "pending" && !existing.notion_page_id) {
    return {
      publicationId: existing.id,
      notionPageId: null,
      notionPageUrl: null,
      status: "in_progress",
    };
  }

  try {
    if (input.reportMode === "live_page" && existing.notion_page_id) {
      await input.publisher.replacePage(existing.notion_page_id, input.report, input.requestId);
      await input.repository.markPublished(
        existing.id,
        existing.notion_page_id,
        existing.notion_page_url,
      );
      return {
        publicationId: existing.id,
        notionPageId: existing.notion_page_id,
        notionPageUrl: existing.notion_page_url,
        status: "updated",
      };
    }

    const page = await input.publisher.createPage(input.report, input.requestId);
    await input.repository.markPublished(existing.id, page.id, page.url);
    return {
      publicationId: existing.id,
      notionPageId: page.id,
      notionPageUrl: page.url,
      status: "published",
    };
  } catch (error) {
    await input.repository.markFailed(existing.id, failureCode(error));
    throw error;
  }
}
