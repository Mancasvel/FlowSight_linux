import { corsHeaders, authenticate, errorResponse, isResponse, jsonResponse } from "../_shared/http.ts";
import {
  decryptTokenBundle,
  NotionApiClient,
  notionDataSourceTitleProperty,
  notionObjectTitle,
} from "../_shared/notion_api.ts";
import { requireNotionPro, sha256Hex } from "../_shared/notion_policy.ts";

function objectSummary(object: Record<string, unknown>, type: "page" | "data_source") {
  return {
    id: typeof object.id === "string" ? object.id : "",
    type,
    title: notionObjectTitle(object),
    url: typeof object.url === "string" ? object.url : null,
    last_edited_time: typeof object.last_edited_time === "string" ? object.last_edited_time : null,
  };
}

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") return new Response("ok", { headers: corsHeaders });
  if (req.method !== "POST") return errorResponse("Method not allowed.", 405, "method_not_allowed");

  const auth = await authenticate(req);
  if (isResponse(auth)) return auth;
  const { data: entitlements, error: entitlementError } = await auth.userClient.rpc(
    "get_user_entitlements",
  );
  if (entitlementError) return errorResponse("Could not verify your plan.", 503, "entitlement_check_failed");
  const gate = requireNotionPro(entitlements);
  if (!gate.allowed) return errorResponse(gate.error, gate.status, gate.code);

  const { data: connection, error: connectionError } = await auth.serviceClient
    .from("notion_connections")
    .select("token_ciphertext,token_iv,encryption_version")
    .eq("user_id", auth.userId)
    .maybeSingle();
  if (connectionError || !connection) {
    return errorResponse("Connect Notion before choosing a destination.", 409, "notion_not_connected");
  }
  const encryptionKey = Deno.env.get("NOTION_TOKEN_ENCRYPTION_KEY");
  if (!encryptionKey) return errorResponse("Notion is not configured.", 503, "notion_not_configured");

  let notion: NotionApiClient;
  try {
    const token = await decryptTokenBundle(connection, encryptionKey);
    notion = new NotionApiClient(token.access_token);
  } catch {
    return errorResponse("The Notion connection must be renewed.", 409, "notion_reconnect_required");
  }

  const body = await req.json().catch(() => ({})) as Record<string, unknown>;
  const action = body.action;
  try {
    if (action === "search") {
      const query = typeof body.query === "string" ? body.query : undefined;
      const [pages, dataSources] = await Promise.all([
        notion.search("page", query),
        notion.search("data_source", query),
      ]);
      const destinations = [
        ...pages.map((item) => objectSummary(item, "page")),
        ...dataSources.map((item) => objectSummary(item, "data_source")),
      ].filter((item) => item.id).sort((a, b) => a.title.localeCompare(b.title));
      return jsonResponse({ destinations });
    }

    if (action === "save") {
      const objectId = typeof body.notion_object_id === "string" ? body.notion_object_id : "";
      const destinationType = body.destination_type === "data_source" ? "data_source" :
        body.destination_type === "page" ? "page" : null;
      const reportMode = body.report_mode === "live_page" ? "live_page" :
        body.report_mode === "period_page" ? "period_page" : null;
      if (!objectId || !destinationType || !reportMode) {
        return errorResponse("Destination and report mode are required.", 400, "invalid_destination");
      }
      const object = destinationType === "data_source"
        ? await notion.retrieveDataSource(objectId)
        : await notion.retrievePage(objectId);
      const titleProperty = destinationType === "data_source"
        ? notionDataSourceTitleProperty(object)
        : null;
      if (destinationType === "data_source" && !titleProperty) {
        return errorResponse("The selected Notion data source needs a title property.", 400, "missing_title_property");
      }

      await auth.serviceClient.from("notion_destinations")
        .update({ is_default: false }).eq("user_id", auth.userId).eq("is_default", true);
      const { data: saved, error } = await auth.serviceClient.from("notion_destinations").upsert({
        user_id: auth.userId,
        notion_object_id: objectId,
        destination_type: destinationType,
        title: notionObjectTitle(object),
        title_property: titleProperty,
        report_mode: reportMode,
        is_default: true,
        updated_at: new Date().toISOString(),
      }, { onConflict: "user_id,notion_object_id" }).select(
        "id,notion_object_id,destination_type,title,report_mode,is_default,updated_at",
      ).single();
      if (error) return errorResponse("Could not save the Notion destination.", 500, "destination_save_failed");
      return jsonResponse({ destination: saved });
    }

    if (action === "create_report_page") {
      const parentPageId = typeof body.parent_page_id === "string" ? body.parent_page_id : "";
      const reportMode = body.report_mode === "live_page" ? "live_page" : "period_page";
      if (!parentPageId) return errorResponse("Choose a parent page.", 400, "parent_page_required");
      await notion.retrievePage(parentPageId);
      const requestId = await sha256Hex(`${auth.userId}:${parentPageId}:flowsight-reports`);
      const page = await notion.createReportContainer(parentPageId, requestId);
      await auth.serviceClient.from("notion_destinations")
        .update({ is_default: false }).eq("user_id", auth.userId).eq("is_default", true);
      const { data: saved, error } = await auth.serviceClient.from("notion_destinations").upsert({
        user_id: auth.userId,
        notion_object_id: page.id,
        destination_type: "page",
        title: "FlowSight Reports",
        title_property: null,
        report_mode: reportMode,
        is_default: true,
        updated_at: new Date().toISOString(),
      }, { onConflict: "user_id,notion_object_id" }).select(
        "id,notion_object_id,destination_type,title,report_mode,is_default,updated_at",
      ).single();
      if (error) return errorResponse("The page was created but could not be selected.", 500, "destination_save_failed");
      return jsonResponse({ destination: saved, notion_page_url: page.url });
    }
  } catch (error) {
    const code = error && typeof error === "object" && "code" in error &&
        typeof (error as { code?: unknown }).code === "string"
      ? (error as { code: string }).code
      : "notion_destination_failed";
    return errorResponse("Notion could not access that destination. Check its sharing permissions.", 502, code);
  }

  return errorResponse("Unknown action.", 400, "invalid_action");
});
