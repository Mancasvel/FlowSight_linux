import { NOTION_API_VERSION } from "./notion_policy.ts";

const NOTION_API_BASE = "https://api.notion.com/v1";

export type NotionTokenBundle = {
  access_token: string;
  refresh_token?: string;
  token_type?: string;
};

export type EncryptedToken = {
  ciphertext: string;
  iv: string;
  version: 1;
};

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value.trim());
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

function asArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return Uint8Array.from(bytes).buffer;
}

function encryptionKeyBytes(encodedKey: string): Uint8Array {
  const bytes = base64ToBytes(encodedKey);
  if (bytes.byteLength !== 32) {
    throw new Error(
      "NOTION_TOKEN_ENCRYPTION_KEY must be a base64-encoded 32-byte key.",
    );
  }
  return bytes;
}

export async function encryptTokenBundle(
  bundle: NotionTokenBundle,
  encodedKey: string,
): Promise<EncryptedToken> {
  if (!bundle.access_token) {
    throw new Error("Notion did not return an access token.");
  }
  const key = await crypto.subtle.importKey(
    "raw",
    asArrayBuffer(encryptionKeyBytes(encodedKey)),
    { name: "AES-GCM" },
    false,
    ["encrypt"],
  );
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(bundle));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv },
    key,
    plaintext,
  );
  return {
    ciphertext: bytesToBase64(new Uint8Array(ciphertext)),
    iv: bytesToBase64(iv),
    version: 1,
  };
}

export async function decryptTokenBundle(
  encrypted: {
    token_ciphertext: string;
    token_iv: string;
    encryption_version: number;
  },
  encodedKey: string,
): Promise<NotionTokenBundle> {
  if (encrypted.encryption_version !== 1) {
    throw new Error("Unsupported token encryption version.");
  }
  const key = await crypto.subtle.importKey(
    "raw",
    asArrayBuffer(encryptionKeyBytes(encodedKey)),
    { name: "AES-GCM" },
    false,
    ["decrypt"],
  );
  const plaintext = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: asArrayBuffer(base64ToBytes(encrypted.token_iv)) },
    key,
    asArrayBuffer(base64ToBytes(encrypted.token_ciphertext)),
  );
  const parsed = JSON.parse(new TextDecoder().decode(plaintext));
  if (!parsed?.access_token || typeof parsed.access_token !== "string") {
    throw new Error("Stored Notion credentials are invalid.");
  }
  return parsed as NotionTokenBundle;
}

export class NotionApiError extends Error {
  constructor(public readonly status: number, public readonly code: string) {
    super(`Notion request failed (${status}, ${code}).`);
  }
}

type Fetcher = typeof fetch;

export class NotionApiClient {
  constructor(
    private readonly accessToken: string,
    private readonly fetcher: Fetcher = fetch,
  ) {}

  private async request(
    path: string,
    init: RequestInit = {},
  ): Promise<Record<string, unknown>> {
    const response = await this.fetcher(`${NOTION_API_BASE}${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${this.accessToken}`,
        "Notion-Version": NOTION_API_VERSION,
        "Content-Type": "application/json",
        ...(init.headers ?? {}),
      },
    });
    const body = await response.json().catch(() => ({})) as Record<
      string,
      unknown
    >;
    if (!response.ok) {
      const code = typeof body.code === "string"
        ? body.code
        : "notion_api_error";
      throw new NotionApiError(response.status, code);
    }
    return body;
  }

  async search(
    object: "page" | "data_source",
    query?: string,
  ): Promise<Record<string, unknown>[]> {
    const body: Record<string, unknown> = {
      page_size: 100,
      sort: { direction: "descending", timestamp: "last_edited_time" },
      filter: { property: "object", value: object },
    };
    if (query?.trim()) body.query = query.trim().slice(0, 100);
    const result = await this.request("/search", {
      method: "POST",
      body: JSON.stringify(body),
    });
    return Array.isArray(result.results)
      ? result.results as Record<string, unknown>[]
      : [];
  }

  retrievePage(pageId: string): Promise<Record<string, unknown>> {
    return this.request(`/pages/${encodeURIComponent(pageId)}`);
  }

  retrieveDataSource(dataSourceId: string): Promise<Record<string, unknown>> {
    return this.request(`/data_sources/${encodeURIComponent(dataSourceId)}`);
  }

  async createChildPage(input: {
    destinationType: "page" | "data_source";
    destinationObjectId: string;
    titleProperty?: string | null;
    title: string;
    blocks: Record<string, unknown>[];
    requestId: string;
  }): Promise<{ id: string; url: string | null }> {
    const parent = input.destinationType === "data_source"
      ? { type: "data_source_id", data_source_id: input.destinationObjectId }
      : { type: "page_id", page_id: input.destinationObjectId };
    const propertyName = input.destinationType === "data_source"
      ? input.titleProperty
      : "title";
    if (!propertyName) {
      throw new Error("The selected Notion data source has no title property.");
    }

    const result = await this.request("/pages", {
      method: "POST",
      headers: { "Idempotency-Key": input.requestId },
      body: JSON.stringify({
        parent,
        properties: {
          [propertyName]: {
            type: "title",
            title: [{
              type: "text",
              text: { content: input.title.slice(0, 2000) },
            }],
          },
        },
        children: input.blocks,
      }),
    });
    if (typeof result.id !== "string") {
      throw new Error("Notion returned a page without an id.");
    }
    return {
      id: result.id,
      url: typeof result.url === "string" ? result.url : null,
    };
  }

  async createReportContainer(
    parentPageId: string,
    requestId: string,
  ): Promise<{ id: string; url: string | null }> {
    return this.createChildPage({
      destinationType: "page",
      destinationObjectId: parentPageId,
      title: "FlowSight Reports",
      blocks: [{
        object: "block",
        type: "paragraph",
        paragraph: {
          rich_text: [{
            type: "text",
            text: {
              content: "Privacy-first work reports published by FlowSight.",
            },
          }],
        },
      }],
      requestId,
    });
  }

  async replacePageMarkdown(
    pageId: string,
    markdown: string,
    requestId: string,
  ): Promise<void> {
    await this.request(`/pages/${encodeURIComponent(pageId)}/markdown`, {
      method: "PATCH",
      headers: { "Idempotency-Key": requestId },
      body: JSON.stringify({
        type: "replace_content",
        replace_content: { new_str: markdown, allow_deleting_content: false },
      }),
    });
  }
}

export async function exchangeAuthorizationCode(input: {
  code: string;
  clientId: string;
  clientSecret: string;
  redirectUri: string;
  fetcher?: Fetcher;
}): Promise<Record<string, unknown>> {
  const fetcher = input.fetcher ?? fetch;
  const basic = btoa(`${input.clientId}:${input.clientSecret}`);
  const response = await fetcher(`${NOTION_API_BASE}/oauth/token`, {
    method: "POST",
    headers: {
      Authorization: `Basic ${basic}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      grant_type: "authorization_code",
      code: input.code,
      redirect_uri: input.redirectUri,
    }),
  });
  const body = await response.json().catch(() => ({})) as Record<
    string,
    unknown
  >;
  if (!response.ok || typeof body.access_token !== "string") {
    throw new Error("Notion OAuth exchange failed.");
  }
  return body;
}

export function notionObjectTitle(object: Record<string, unknown>): string {
  const directTitle = object.title;
  if (Array.isArray(directTitle)) {
    const title = directTitle.map((part) => {
      if (!part || typeof part !== "object") return "";
      const value = part as Record<string, unknown>;
      return typeof value.plain_text === "string" ? value.plain_text : "";
    }).join("").trim();
    if (title) return title;
  }
  const properties = object.properties;
  if (
    properties && typeof properties === "object" && !Array.isArray(properties)
  ) {
    for (const value of Object.values(properties as Record<string, unknown>)) {
      if (!value || typeof value !== "object" || Array.isArray(value)) continue;
      const property = value as Record<string, unknown>;
      if (property.type !== "title" || !Array.isArray(property.title)) continue;
      const title = property.title.map((part) => {
        if (!part || typeof part !== "object") return "";
        const text = part as Record<string, unknown>;
        return typeof text.plain_text === "string" ? text.plain_text : "";
      }).join("").trim();
      if (title) return title;
    }
  }
  return "Untitled";
}

export function notionDataSourceTitleProperty(
  object: Record<string, unknown>,
): string | null {
  const properties = object.properties;
  if (
    !properties || typeof properties !== "object" || Array.isArray(properties)
  ) return null;
  for (
    const [name, value] of Object.entries(properties as Record<string, unknown>)
  ) {
    if (
      value && typeof value === "object" && !Array.isArray(value) &&
      (value as Record<string, unknown>).type === "title"
    ) return name;
  }
  return null;
}
