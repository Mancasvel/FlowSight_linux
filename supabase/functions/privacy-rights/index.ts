import {
  authenticate,
  corsHeaders,
  errorResponse,
  isResponse,
  jsonResponse,
} from "../_shared/http.ts";
import type { SupabaseClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";
import {
  assertSafePrivacyExportProjections,
  CURRENT_PRIVACY_NOTICE_VERSION,
  PRIVACY_EXPORT_SOURCES,
  type PrivacyExportSource,
} from "../_shared/privacy_policy.ts";

const PAGE_SIZE = 1000;
assertSafePrivacyExportProjections();

function missingRelation(
  error: { code?: string; message?: string } | null,
): boolean {
  return Boolean(
    error &&
      (error.code === "42P01" || error.code === "PGRST205" ||
        String(error.message ?? "").includes("schema cache")),
  );
}

async function exportRows(
  serviceClient: SupabaseClient,
  source: PrivacyExportSource,
  userId: string,
) {
  const rows: unknown[] = [];
  for (let offset = 0;; offset += PAGE_SIZE) {
    const { data, error } = await serviceClient
      .from(source.table)
      .select(source.select ?? "*")
      .eq(source.column, userId)
      .range(offset, offset + PAGE_SIZE - 1);
    if (missingRelation(error)) return [];
    if (error) throw new Error(`Could not export ${source.table}.`);
    const page = data ?? [];
    rows.push(...page);
    if (page.length < PAGE_SIZE) return rows;
  }
}

async function deleteRows(
  serviceClient: SupabaseClient,
  source: PrivacyExportSource,
  userId: string,
) {
  const { error } = await serviceClient.from(source.table).delete().eq(
    source.column,
    userId,
  );
  if (missingRelation(error)) return;
  if (error) throw new Error(`Could not erase ${source.table}.`);
}

async function sha256(value: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

async function readPseudonymousData(
  serviceClient: SupabaseClient,
  anonymousId: unknown,
  anonymousSecret: unknown,
) {
  if (anonymousId == null && anonymousSecret == null) {
    return { analytics: null, feedback: [] };
  }
  const id = String(anonymousId ?? "");
  const secret = String(anonymousSecret ?? "");
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
      .test(id) ||
    secret.length < 32 || secret.length > 256
  ) {
    throw new Error("The pseudonymous analytics credential is invalid.");
  }
  const suppliedHash = await sha256(secret);
  const { data: credential, error: credentialError } = await serviceClient
    .from("anonymous_product_analytics")
    .select("secret_hash")
    .eq("anonymous_id", id)
    .maybeSingle();
  if (credentialError) {
    throw new Error("Could not verify pseudonymous analytics data.");
  }
  if (!credential) return { analytics: null, feedback: [] };

  // Claim rows written by the pre-credential release. Possession of the
  // unguessable installation UUID is the only proof available for that
  // one-time migration; all subsequent operations require the secret.
  if (credential.secret_hash == null) {
    const { error } = await serviceClient.from("anonymous_product_analytics")
      .update({ secret_hash: suppliedHash })
      .eq("anonymous_id", id)
      .is("secret_hash", null);
    if (error) {
      throw new Error("Could not migrate pseudonymous analytics data.");
    }
  } else if (credential.secret_hash !== suppliedHash) {
    throw new Error("The pseudonymous analytics credential is invalid.");
  }

  const analyticsResult = await serviceClient.from(
    "anonymous_product_analytics",
  )
    .select(
      "anonymous_id,daily_usage,weekly_primary_activity,updated_at,expires_at",
    )
    .eq("anonymous_id", id)
    .maybeSingle();
  if (analyticsResult.error) {
    throw new Error("Could not export pseudonymous analytics data.");
  }
  const feedbackResult = await serviceClient.from("product_feedback")
    .select("id,anonymous_id,message,app_version,created_at,expires_at")
    .eq("anonymous_id", id);
  if (feedbackResult.error) {
    throw new Error("Could not export pseudonymous feedback data.");
  }
  return {
    analytics: analyticsResult.data,
    feedback: feedbackResult.data ?? [],
  };
}

async function deletePseudonymousData(
  serviceClient: SupabaseClient,
  anonymousId: unknown,
  anonymousSecret: unknown,
) {
  const data = await readPseudonymousData(
    serviceClient,
    anonymousId,
    anonymousSecret,
  );
  if (!data.analytics) return;
  const id = String(anonymousId);
  const feedbackDelete = await serviceClient.from("product_feedback")
    .delete()
    .eq("anonymous_id", id);
  if (feedbackDelete.error) {
    throw new Error("Could not erase pseudonymous feedback data.");
  }
  const analyticsDelete = await serviceClient.from(
    "anonymous_product_analytics",
  )
    .delete()
    .eq("anonymous_id", id);
  if (analyticsDelete.error) {
    throw new Error("Could not erase pseudonymous analytics data.");
  }
}

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") {
    return new Response("ok", { headers: corsHeaders });
  }
  if (req.method !== "POST") {
    return errorResponse("Method not allowed.", 405, "method_not_allowed");
  }

  const auth = await authenticate(req);
  if (isResponse(auth)) return auth;
  const body = await req.json().catch(() => ({})) as Record<string, unknown>;
  const action = String(body.action ?? "");

  try {
    if (action === "update_preferences") {
      if (body.notice_version !== CURRENT_PRIVACY_NOTICE_VERSION) {
        return errorResponse(
          "Review the current privacy notice before enabling cloud processing.",
          409,
          "privacy_notice_required",
        );
      }
      const { error } = await auth.serviceClient.from("privacy_preferences")
        .upsert({
          user_id: auth.userId,
          notice_version: CURRENT_PRIVACY_NOTICE_VERSION,
          cloud_sync_enabled: body.cloud_sync_enabled === true,
          cloud_ai_enabled: body.cloud_ai_enabled === true,
          updated_at: new Date().toISOString(),
        }, { onConflict: "user_id" });
      if (error) throw new Error("Could not save privacy preferences.");
      return jsonResponse({ saved: true });
    }

    if (action === "export") {
      const { data: account, error: accountError } = await auth.serviceClient
        .auth.admin
        .getUserById(auth.userId);
      if (accountError || !account.user) {
        throw new Error("Could not export account metadata.");
      }

      const data: Record<string, unknown[]> = {};
      for (const source of PRIVACY_EXPORT_SOURCES) {
        data[source.table] = await exportRows(
          auth.serviceClient,
          source,
          auth.userId,
        );
      }
      const invitationRows = account.user.email
        ? await auth.serviceClient.from("invitations")
          .select("team_id,expires_at,used_at,created_by,email")
          .eq("email", account.user.email)
        : { data: [], error: null };
      if (!missingRelation(invitationRows.error) && invitationRows.error) {
        throw new Error("Could not export invitations.");
      }
      data.invitations = invitationRows.data ?? [];
      const createdInvitations = await auth.serviceClient.from("invitations")
        .select("team_id,expires_at,used_at,created_by,email")
        .eq("created_by", auth.userId);
      if (
        !missingRelation(createdInvitations.error) && createdInvitations.error
      ) {
        throw new Error(
          "Could not export invitations created by this account.",
        );
      }
      data.invitations_created = createdInvitations.data ?? [];
      const pseudonymousData = await readPseudonymousData(
        auth.serviceClient,
        body.anonymous_id,
        body.anonymous_secret,
      );

      return jsonResponse({
        generated_at: new Date().toISOString(),
        account: {
          id: account.user.id,
          email: account.user.email,
          created_at: account.user.created_at,
          updated_at: account.user.updated_at,
          last_sign_in_at: account.user.last_sign_in_at,
          providers: account.user.app_metadata?.providers ?? [],
          profile_metadata: account.user.user_metadata ?? {},
        },
        data,
        pseudonymous_data: pseudonymousData,
        note:
          "Authentication credentials and encryption material are excluded for security.",
      }, 200);
    }

    if (action === "delete_account") {
      if (body.confirmation !== "DELETE") {
        return errorResponse(
          "Explicit deletion confirmation is required.",
          400,
          "confirmation_required",
        );
      }

      // Never let deleting an owner silently erase other team members' data or
      // active billing relationships through legacy ON DELETE CASCADE keys.
      for (
        const ownership of [
          {
            table: "teams",
            message:
              "Transfer or delete teams you own before deleting this account.",
          },
          {
            table: "licenses",
            message:
              "Resolve the subscription or licence you own before deleting this account.",
          },
        ]
      ) {
        const owned = await auth.serviceClient.from(ownership.table)
          .select("id")
          .eq("owner_id", auth.userId)
          .limit(1);
        if (!missingRelation(owned.error) && owned.error) {
          throw new Error(`Could not verify ${ownership.table} ownership.`);
        }
        if ((owned.data ?? []).length > 0) {
          return errorResponse(
            ownership.message,
            409,
            "ownership_transfer_required",
          );
        }
      }

      const { data: account, error: accountError } = await auth.serviceClient
        .auth.admin
        .getUserById(auth.userId);
      if (accountError || !account.user) {
        throw new Error("Could not verify the account.");
      }
      await deletePseudonymousData(
        auth.serviceClient,
        body.anonymous_id,
        body.anonymous_secret,
      );

      // Delete child/user-owned data first so deployments whose older foreign
      // keys lack ON DELETE CASCADE are still erasable. Billing records are not
      // listed here: statutory accounting retention must be handled by the
      // controller's billing system and documented separately.
      for (const source of [...PRIVACY_EXPORT_SOURCES].reverse()) {
        await deleteRows(auth.serviceClient, source, auth.userId);
      }
      if (account.user.email) {
        const receivedInvitations = await auth.serviceClient.from("invitations")
          .delete()
          .eq("email", account.user.email);
        if (
          !missingRelation(receivedInvitations.error) &&
          receivedInvitations.error
        ) {
          throw new Error("Could not erase received invitations.");
        }
      }
      const createdInvitations = await auth.serviceClient.from("invitations")
        .delete()
        .eq("created_by", auth.userId);
      if (
        !missingRelation(createdInvitations.error) && createdInvitations.error
      ) {
        throw new Error("Could not erase created invitations.");
      }
      const { error } = await auth.serviceClient.auth.admin.deleteUser(
        auth.userId,
      );
      if (error) {
        throw new Error(
          "Personal data was removed, but the login account could not be deleted.",
        );
      }
      return jsonResponse({ deleted: true });
    }

    return errorResponse("Unknown action.", 400, "invalid_action");
  } catch (error) {
    // Do not expose database structure, tokens, or vendor response bodies.
    return errorResponse(
      error instanceof Error ? error.message : "Privacy request failed.",
      500,
      "privacy_request_failed",
    );
  }
});
